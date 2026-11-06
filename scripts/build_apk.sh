#!/usr/bin/env bash
# BLACK WOOF Stage 2 build, no Gradle:
#   cargo -> javac -> d8 -> aapt2 -> zip -> zipalign (16 KB) -> apksigner
#
# Needs: ANDROID_HOME, ANDROID_NDK_HOME, Rust with the aarch64-linux-android target, JDK 17+.
# Optional:
#   REPEAT_CHECK=1   build the native library and the dex twice in this run and compare their
#                    SHA-256 (intra-run repeatability of those two files; the APK itself is
#                    NOT compared, so this is not a proof of full APK reproducibility)
#   KEYSTORE_B64     base64 of a .jks (stable signing key); KEYSTORE_PASS (required with it),
#                    KEY_ALIAS. Without it a throw-away key is generated, so every build has a
#                    different signature (uninstall the old build before installing a new one).
# Signing material is created in RUNNER_TEMP (or TMPDIR, or /tmp), never inside the repository,
# is passed to keytool/apksigner through the environment, is never printed, and is removed by an
# EXIT trap on success and on failure.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${ANDROID_HOME:?ANDROID_HOME is not set}"
: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME is not set}"

BUILD_TOOLS_VERSION="${BUILD_TOOLS_VERSION:-35.0.0}"
PLATFORM_API="${PLATFORM_API:-35}"
MIN_SDK=28
TARGET_SDK=35
VERSION_CODE=2
VERSION_NAME="0.2.0-stage2"
TARGET="aarch64-linux-android"
APK_NAME="BlackWoof-stage2"
LIB="libblackwoof.so"

BT="$ANDROID_HOME/build-tools/$BUILD_TOOLS_VERSION"
ANDROID_JAR="$ANDROID_HOME/platforms/android-$PLATFORM_API/android.jar"
TOOLCHAIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64"
OUT="$ROOT/build"

export TZ=UTC
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || echo 315532800)}"
export SOURCE_DATE_EPOCH

for tool in "$BT/aapt2" "$BT/d8" "$BT/zipalign" "$BT/apksigner" "$ANDROID_JAR" \
            "$TOOLCHAIN/bin/aarch64-linux-android${MIN_SDK}-clang"; do
  [ -e "$tool" ] || { echo "ERROR: missing $tool"; exit 1; }
done
command -v keytool >/dev/null || { echo "ERROR: keytool (JDK) not on PATH"; exit 1; }

LOCKED=""
if [ -f "$ROOT/Cargo.lock" ]; then
  LOCKED="--locked"
else
  echo "WARNING: Cargo.lock is not committed; dependency versions are not pinned."
fi

hash_of() { sha256sum "$1" | cut -d' ' -f1; }

build_native() {
  export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/bin/aarch64-linux-android${MIN_SDK}-clang"
  local cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  local sysroot
  sysroot="$(rustc --print sysroot)"
  # 16 KB ELF segment alignment (NDK r28+ does it by default; stated explicitly on purpose).
  # remap-path-prefix keeps build machine paths out of the binary (reproducibility).
  export RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384 \
--remap-path-prefix=$ROOT=/blackwoof \
--remap-path-prefix=$cargo_home=/cargo \
--remap-path-prefix=$sysroot=/rustc"
  ( cd "$ROOT" && cargo build --release --target "$TARGET" $LOCKED )
}

build_dex() {
  local dir="$1"
  rm -rf "$dir"
  mkdir -p "$dir/classes" "$dir/dex"
  local sources classes
  sources="$(find "$ROOT/java" -name '*.java' | LC_ALL=C sort)"
  # shellcheck disable=SC2086
  javac -source 8 -target 8 -Xlint:-options -encoding UTF-8 \
        -bootclasspath "$ANDROID_JAR" -d "$dir/classes" $sources
  classes="$(find "$dir/classes" -name '*.class' | LC_ALL=C sort)"
  # shellcheck disable=SC2086
  "$BT/d8" --release --min-api "$MIN_SDK" --lib "$ANDROID_JAR" --output "$dir/dex" $classes
}

# Temporary signing material: outside the repository, removed on every exit path.
SIGN_DIR=""
cleanup() {
  if [ -n "$SIGN_DIR" ]; then
    rm -rf "$SIGN_DIR"
  fi
  rm -f "$OUT"/*.idsig
}
trap cleanup EXIT

rm -rf "$OUT"
mkdir -p "$OUT"

echo "== native ($TARGET, release)"
build_native
SO="$ROOT/target/$TARGET/release/$LIB"
cp "$SO" "$OUT/$LIB"

echo "== java -> dex"
build_dex "$OUT/java1"

if [ "${REPEAT_CHECK:-0}" = "1" ]; then
  echo "== repeatability check (native library and DEX only): second clean build"
  so1="$(hash_of "$SO")"
  dex1="$(hash_of "$OUT/java1/dex/classes.dex")"
  ( cd "$ROOT" && cargo clean --release --target "$TARGET" $LOCKED )
  build_native
  build_dex "$OUT/java2"
  so2="$(hash_of "$SO")"
  dex2="$(hash_of "$OUT/java2/dex/classes.dex")"
  [ "$so1" = "$so2" ] || { echo "REPEAT FAIL: $LIB differs ($so1 vs $so2)"; exit 1; }
  [ "$dex1" = "$dex2" ] || { echo "REPEAT FAIL: classes.dex differs ($dex1 vs $dex2)"; exit 1; }
  echo "REPEAT OK: $LIB sha256=$so1 (same in both builds of this run)"
  echo "REPEAT OK: classes.dex sha256=$dex1 (same in both builds of this run)"
  echo "REPEAT NOTE: the APK is not compared; this is not full APK reproducibility"
fi

echo "== package"
STAGE="$OUT/stage"
mkdir -p "$STAGE/lib/arm64-v8a"
cp "$OUT/java1/dex/classes.dex" "$STAGE/classes.dex"
cp "$OUT/$LIB" "$STAGE/lib/arm64-v8a/$LIB"
find "$STAGE" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +

"$BT/aapt2" link -o "$OUT/base.apk" \
  --manifest "$ROOT/android/AndroidManifest.xml" -I "$ANDROID_JAR" \
  --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
  --version-code "$VERSION_CODE" --version-name "$VERSION_NAME"

cp "$OUT/base.apk" "$OUT/unaligned.apk"
( cd "$STAGE" \
  && zip -q -X -D "$OUT/unaligned.apk" classes.dex \
  && zip -q -X -D -0 "$OUT/unaligned.apk" "lib/arm64-v8a/$LIB" )

# -P 16: page-align the stored .so files for 16 KB devices.
"$BT/zipalign" -f -P 16 4 "$OUT/unaligned.apk" "$OUT/aligned.apk"

echo "== sign"
SIGN_DIR="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/blackwoof-sign.XXXXXX")"
chmod 700 "$SIGN_DIR"
case "$SIGN_DIR" in
  "$ROOT"|"$ROOT"/*) echo "ERROR: signing directory is inside the repository"; exit 1 ;;
esac
KS="$SIGN_DIR/signing.jks"
KEY_ALIAS="${KEY_ALIAS:-blackwoof}"
if [ -n "${KEYSTORE_B64:-}" ]; then
  [ -n "${KEYSTORE_PASS:-}" ] || { echo "ERROR: KEYSTORE_PASS is required together with KEYSTORE_B64"; exit 1; }
  ( umask 077; printf '%s' "$KEYSTORE_B64" | base64 -d > "$KS" )
  export BW_KS_PASS="$KEYSTORE_PASS"
else
  # Throw-away key with a random password. Passwords go to the tools through the environment,
  # never on a command line and never to the log.
  export BW_KS_PASS="$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  if ! keytool -genkeypair -keystore "$KS" -storepass:env BW_KS_PASS -keypass:env BW_KS_PASS \
      -alias "$KEY_ALIAS" -keyalg RSA -keysize 2048 -validity 10000 \
      -dname "CN=BLACK WOOF test build" > /dev/null 2> "$SIGN_DIR/keytool.err"; then
    echo "ERROR: keytool failed:"
    cat "$SIGN_DIR/keytool.err"
    exit 1
  fi
fi
"$BT/apksigner" sign --ks "$KS" --ks-pass env:BW_KS_PASS --key-pass env:BW_KS_PASS \
  --ks-key-alias "$KEY_ALIAS" --min-sdk-version "$MIN_SDK" \
  --out "$OUT/$APK_NAME.apk" "$OUT/aligned.apk"
"$BT/apksigner" verify --verbose "$OUT/$APK_NAME.apk"
# Remove the key as early as possible; the EXIT trap remains the safety net.
rm -rf "$SIGN_DIR"
SIGN_DIR=""
unset BW_KS_PASS

echo "BUILD OK: $OUT/$APK_NAME.apk"
