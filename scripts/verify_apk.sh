#!/usr/bin/env bash
# Verifies a built BLACK WOOF APK. Prints OK/FAIL lines; exits non-zero on the first FAIL.
# Usage: scripts/verify_apk.sh build/BlackWoof-stage2.apk
set -euo pipefail

APK="${1:?usage: verify_apk.sh <apk>}"
: "${ANDROID_HOME:?ANDROID_HOME is not set}"
: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME is not set}"

BUILD_TOOLS_VERSION="${BUILD_TOOLS_VERSION:-35.0.0}"
BT="$ANDROID_HOME/build-tools/$BUILD_TOOLS_VERSION"
TOOLCHAIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64"
READELF="$TOOLCHAIN/bin/llvm-readelf"
NM="$TOOLCHAIN/bin/llvm-nm"
LIB="libblackwoof.so"
JNI_PREFIX="Java_com_dzekiwolf_blackwoof_Native_"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

ok() { echo "OK:   $*"; }
fail() { echo "FAIL: $*"; exit 1; }

[ -f "$APK" ] || fail "APK not found: $APK"

# --- permissions: none at all in Stage 2, and never INTERNET -------------------------------
PERMS="$("$BT/aapt2" dump permissions "$APK")"
echo "$PERMS"
if grep -q 'android.permission.INTERNET' <<<"$PERMS"; then fail "INTERNET permission present"; fi
# Since Stage 3 the allowed set is exactly the two foreground-service permissions.
PERM_NAMES="$(sed -n "s/^uses-permission[a-z0-9-]*: name='\([^']*\)'.*/\1/p" <<<"$PERMS" | LC_ALL=C sort)"
EXPECTED_PERMS="android.permission.FOREGROUND_SERVICE
android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK"
if [ "$PERM_NAMES" != "$EXPECTED_PERMS" ]; then
  echo "found:    $(tr '\n' ' ' <<<"$PERM_NAMES")"
  echo "expected: $(tr '\n' ' ' <<<"$EXPECTED_PERMS")"
  fail "requested permissions differ from the allowed set"
fi
ok "permissions are exactly FOREGROUND_SERVICE and FOREGROUND_SERVICE_MEDIA_PLAYBACK (no INTERNET)"

# --- badging: SDK levels and ABI ------------------------------------------------------------
BADGING="$("$BT/aapt2" dump badging "$APK")"
grep -E "^(package|minSdkVersion|targetSdkVersion|native-code|application-label):" <<<"$BADGING" || true
EXPECTED_PACKAGE="com.dzekiwolf.blackwoof"
grep -Eq "^package: name='${EXPECTED_PACKAGE//./\\.}'( |\$)" <<<"$BADGING" \
  || fail "package name is not $EXPECTED_PACKAGE"
ok "package name is $EXPECTED_PACKAGE"
grep -q "^minSdkVersion:'28'" <<<"$BADGING" || fail "minSdk is not 28"
grep -q "^targetSdkVersion:'35'" <<<"$BADGING" || fail "targetSdk is not 35"
grep -q "^native-code: 'arm64-v8a'$" <<<"$BADGING" || fail "native-code is not exactly arm64-v8a"
ok "minSdk 28, targetSdk 35, ABI arm64-v8a only"

# --- zip layout: 16 KB alignment, native library stored (not compressed) ---------------------
# One strict check. A failed alignment check ends the verification with a non-zero exit code.
ALIGN_OUT="$("$BT/zipalign" -c -P 16 -v 4 "$APK" 2>&1)" || {
  tail -n 5 <<<"$ALIGN_OUT"
  fail "zipalign: APK is not 16 KB aligned"
}
tail -n 3 <<<"$ALIGN_OUT"
LISTING="$(unzip -v "$APK")"
LIB_LINE="$(grep "lib/arm64-v8a/$LIB" <<<"$LISTING" || true)"
grep -q 'Stored' <<<"$LIB_LINE" || fail "$LIB is compressed inside the APK"
ok "APK is 16 KB aligned and $LIB is stored uncompressed"

# --- signature -------------------------------------------------------------------------------
SIGNATURE="$("$BT/apksigner" verify --verbose "$APK")" || fail "APK signature does not verify"
head -n 6 <<<"$SIGNATURE"
ok "APK signature verifies"

# --- native library: architecture, ELF alignment, dependencies, symbols ---------------------
SO="$TMP/$LIB"
unzip -p "$APK" "lib/arm64-v8a/$LIB" > "$SO"

grep -q 'Machine:.*AArch64' <<<"$("$READELF" -h "$SO")" || fail "$LIB is not AArch64"
ok "$LIB is AArch64"

while read -r align; do
  [ $((align)) -ge 16384 ] || fail "ELF LOAD segment alignment $align is below 16 KB"
done < <("$READELF" -lW "$SO" | awk '$1 == "LOAD" { print $NF }')
ok "all ELF LOAD segments are aligned to at least 16 KB"

NEEDED="$("$READELF" -d "$SO" | grep NEEDED | sed 's/.*\[\(.*\)\]/\1/' | sort)"
echo "NEEDED libraries:"
echo "$NEEDED"
ALLOWED=" libaaudio.so libc.so libm.so libdl.so "
for lib in $NEEDED; do
  case "$ALLOWED" in
    *" $lib "*) ;;
    *) fail "unexpected shared library dependency: $lib" ;;
  esac
done
grep -q '^libaaudio.so$' <<<"$NEEDED" || fail "libaaudio.so is not linked"
ok "dependencies are limited to libaaudio, libc, libm, libdl"

EXPORTS="$("$NM" -D --defined-only "$SO" | awk '{ print $3 }' | grep "^$JNI_PREFIX" | sort || true)"
for name in nativeApiVersion nativePlay nativePause nativeResume nativeStop nativeState nativeIsActive nativeShutdown; do
  grep -qx "$JNI_PREFIX$name" <<<"$EXPORTS" || fail "missing JNI export: $name"
done
ok "all 8 JNI exports are present"
ALL_DEFINED="$("$NM" -D --defined-only "$SO" | awk '{ print $3 }')"
grep -qx 'JNI_OnLoad' <<<"$ALL_DEFINED" || fail "missing JNI_OnLoad (Rust to Java callbacks)"
ok "JNI_OnLoad is present"

UNDEFINED="$("$NM" -D --undefined-only "$SO")"
if grep -qi '__android_log' <<<"$UNDEFINED"; then
  fail "the library references the Android logging API"
fi
ok "no reference to the Android logging API"

# --- sizes -----------------------------------------------------------------------------------
echo "SIZE apk:                  $(stat -c %s "$APK") bytes"
echo "SIZE $LIB (in apk): $(stat -c %s "$SO") bytes"
echo "SIZE classes.dex (in apk): $(unzip -p "$APK" classes.dex | wc -c) bytes"
echo "ALL CHECKS PASSED"
