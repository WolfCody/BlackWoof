# BLACK WOOF

Максимально лёгкий офлайн-плеер для Android, написанный преимущественно на Rust.

**RUST FIRST. AUDIO FIRST. OFFLINE FIRST. ZERO-FRILLS. НИКАКИХ ЛОГОВ.**

## Статус

**Проверки Stage 2 в CI пройдены.** GitHub Actions для коммита [`5305959`](https://github.com/WolfCody/BlackWoof/commit/5305959) (`Add Cargo lockfile`) завершился со статусом `Success`.

- [Успешный запуск GitHub Actions](https://github.com/WolfCody/BlackWoof/actions/runs/42223340495)
- Владелец сообщил, что установил APK на Android 15 / ARM64, приложение запускается и временный тестовый интерфейс Stage 2 работает.

Это подтверждает CI-сборку и базовую проверку тестового стенда, но **не означает, что уже реализован полноценный пользовательский музыкальный плеер**. Список реализованных и отсутствующих функций приведён в [docs/STAGE2.md](docs/STAGE2.md).

## Проверки CI

Успешный запуск CI подтвердил:

- выполнение Rust host tests;
- сборку APK для `arm64-v8a`;
- проверку подписи APK;
- `minSdk 28`, `targetSdk 35`;
- отсутствие запрашиваемых разрешений, включая `INTERNET`;
- 16 KB alignment APK и ELF LOAD segments;
- наличие всех 7 ожидаемых JNI exports;
- отсутствие Android logging API references;
- нативные зависимости: `libaaudio`, `libc`, `libm`, `libdl`;
- repeatability SHA-256 для `libblackwoof.so` и `classes.dex` в пределах одного запуска.

Размеры из успешного CI-запуска:

| Артефакт | Размер |
|---|---:|
| APK | 954,846 bytes |
| `libblackwoof.so` | 929,176 bytes |
| `classes.dex` | 7,580 bytes |

Проверка повторяемости `.so` и DEX внутри одного запуска **не является доказательством полной воспроизводимости APK**: сам APK этим сравнением не проверяется.

## Сборка и проверки

Сборка выполняется GitHub Actions без Gradle: Rust → `javac` → `d8` → `aapt2` → `zip` → `zipalign` → `apksigner`.

Для локального запуска нужны Rust toolchain, Android SDK/NDK и JDK 17+:

```bash
cargo test --lib --locked
ANDROID_HOME=... ANDROID_NDK_HOME=... bash scripts/build_apk.sh
ANDROID_HOME=... ANDROID_NDK_HOME=... bash scripts/verify_apk.sh build/BlackWoof-stage2.apk
```

## Cargo.lock

`Cargo.lock` **закоммичен в `main`** отдельным коммитом [`5305959`](https://github.com/WolfCody/BlackWoof/commit/5305959). Он фиксирует разрешённые версии зависимостей. Не удаляйте и не пересоздавайте его без отдельного обоснованного изменения зависимостей.

## Подпись

В CI временный ключ создаётся в `RUNNER_TEMP` вне репозитория, пароль передаётся через окружение и не печатается. Ключ удаляется `EXIT` trap при успехе и при ошибке; signing key не включается в артефакты.

## Текущие ограничения

Stage 2 — это аудиодвижок, декодер, Android audio output и временный тестовый экран. Пока не реализованы внутренняя музыкальная библиотека, очередь и навигация по трекам, Shuffle/Repeat, seek, сохранение позиции, Foreground Service, MediaSession и системное медианотификейшен. Фоновое воспроизведение не заявляется как поддерживаемая функция.

Следующий этап разработки остаётся заблокирован до принятия владельцем финального отчёта Stage 2 и отдельного разрешения на Stage 3.
