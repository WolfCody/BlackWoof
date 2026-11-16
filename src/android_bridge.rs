//! JNI surface (Android only).
//!
//! Java -> Rust: exports take primitives and strings, and every body runs inside
//! `catch_unwind`, so a panic never unwinds through the `extern "system"` boundary.
//!
//! Rust -> Java: `JNI_OnLoad` caches the `JavaVM` and global references to the three Java glue
//! classes (the app class loader is only reachable from there). `AndroidPlatform` is the
//! `Platform` the controller uses; each call is best effort and a Java exception is cleared,
//! so the platform side can never disturb playback.
//!
//! Descriptor contract: `Platform.openAudio` returns a descriptor from `detachFd()`; Rust
//! takes ownership at once (see `fd::take_fd`), so Java never closes it afterwards.

// JNI export names are dictated by the JVM and cannot be snake_case.
#![allow(non_snake_case)]

use std::os::raw::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};

use jni::objects::{GlobalRef, JClass, JString, JValue};
use jni::sys::{jint, jstring, JNI_ERR, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};

use crate::aaudio::AAudioSink;
use crate::audio_engine::{Engine, Sink};
use crate::guard::silence_panics;
use crate::playback_controller::{Controller, Followup, Platform};

/// Increase when the Java <-> native contract changes.
const API_VERSION: i32 = 4;

type JniEnv = *mut c_void;
type JniClass = *mut c_void;

struct Classes {
    service: GlobalRef,
    activity: GlobalRef,
    platform: GlobalRef,
}

static VM: OnceLock<JavaVM> = OnceLock::new();
static CLASSES: OnceLock<Classes> = OnceLock::new();
static CONTROLLER: OnceLock<Controller> = OnceLock::new();

fn controller() -> &'static Controller {
    CONTROLLER.get_or_init(|| {
        let engine = Engine::new(Arc::new(|| -> Box<dyn Sink> { Box::new(AAudioSink::new()) }));
        engine.set_observer(Arc::new(|state, error| {
            if let Some(Followup::Advance) = controller().on_engine_event(state, error) {
                // Starting the next track joins the thread that is calling: use another one.
                let _ = std::thread::Builder::new()
                    .name("bw-advance".to_string())
                    .spawn(|| controller().advance());
            }
        }));
        Controller::new(engine, Arc::new(AndroidPlatform))
    })
}

/// Runs once when the library is loaded. Failing here makes `System.loadLibrary` throw, which
/// the Java side turns into "engine not loaded" instead of a silently broken app.
#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: JavaVM, _reserved: *mut c_void) -> jint {
    silence_panics();
    let prepared = catch_unwind(AssertUnwindSafe(|| {
        let mut env = vm.get_env().ok()?;
        let mut find = |name: &str| -> Option<GlobalRef> {
            let class = env.find_class(name).ok()?;
            env.new_global_ref(&class).ok()
        };
        Some(Classes {
            service: find("com/dzekiwolf/blackwoof/PlayerService")?,
            activity: find("com/dzekiwolf/blackwoof/MainActivity")?,
            platform: find("com/dzekiwolf/blackwoof/Platform")?,
        })
    }));
    match prepared {
        Ok(Some(classes)) => {
            let _ = CLASSES.set(classes);
            let _ = VM.set(vm);
            JNI_VERSION_1_6
        }
        _ => JNI_ERR,
    }
}

// ---- Rust -> Java ---------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Target {
    Service,
    Activity,
    Platform,
}

/// Runs `f` with an attached JNI environment. A Java exception is cleared afterwards.
fn with_env<R>(f: impl FnOnce(&mut JNIEnv, &Classes) -> Option<R>) -> Option<R> {
    let vm = VM.get()?;
    let classes = CLASSES.get()?;
    let mut guard = vm.attach_current_thread().ok()?;
    let result = f(&mut guard, classes);
    if guard.exception_check().unwrap_or(false) {
        let _ = guard.exception_clear();
    }
    result
}

fn class_of(global: &GlobalRef) -> JClass<'static> {
    // SAFETY: the global reference keeps the class object alive; this wrapper does not own it
    // and dropping the wrapper releases nothing.
    unsafe { JClass::from_raw(global.as_obj().as_raw()) }
}

fn call_void(target: Target, name: &str) {
    let _ = with_env(|env, c| {
        let global = match target {
            Target::Service => &c.service,
            Target::Activity => &c.activity,
            Target::Platform => &c.platform,
        };
        env.call_static_method(&class_of(global), name, "()V", &[]).ok()?;
        Some(())
    });
}

struct AndroidPlatform;

impl Platform for AndroidPlatform {
    fn open_fd(&self, uri: &str) -> Option<i32> {
        with_env(|env, c| {
            let uri = env.new_string(uri).ok()?;
            let fd = env
                .call_static_method(
                    &class_of(&c.platform),
                    "openAudio",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&uri)],
                )
                .ok()?
                .i()
                .ok()?;
            if fd >= 0 {
                Some(fd)
            } else {
                None
            }
        })
    }

    fn query_tracks(&self) -> Option<String> {
        with_env(|env, c| {
            let value = env
                .call_static_method(&class_of(&c.platform), "queryTracks", "()Ljava/lang/String;", &[])
                .ok()?;
            let object = value.l().ok()?;
            if object.is_null() {
                return None;
            }
            let text = JString::from(object);
            let text: String = env.get_string(&text).ok()?.into();
            Some(text)
        })
    }

    fn has_audio_permission(&self) -> bool {
        with_env(|env, c| {
            env.call_static_method(&class_of(&c.platform), "hasAudioPermission", "()Z", &[])
                .ok()?
                .z()
                .ok()
        })
        .unwrap_or(false)
    }

    fn display_name(&self, uri: &str) -> Option<String> {
        with_env(|env, c| {
            let uri = env.new_string(uri).ok()?;
            let value = env
                .call_static_method(
                    &class_of(&c.platform),
                    "displayName",
                    "(Ljava/lang/String;)Ljava/lang/String;",
                    &[JValue::Object(&uri)],
                )
                .ok()?;
            let object = value.l().ok()?;
            if object.is_null() {
                return None;
            }
            let text = JString::from(object);
            let text: String = env.get_string(&text).ok()?.into();
            Some(text)
        })
    }

    fn show_session(&self, state: i32, title: &str, artist: &str, actions: i32) {
        let _ = with_env(|env, c| {
            let title = env.new_string(title).ok()?;
            let artist = env.new_string(artist).ok()?;
            env.call_static_method(
                &class_of(&c.service),
                "show",
                "(ILjava/lang/String;Ljava/lang/String;I)V",
                &[
                    JValue::Int(state),
                    JValue::Object(&title),
                    JValue::Object(&artist),
                    JValue::Int(actions),
                ],
            )
            .ok()?;
            Some(())
        });
    }

    fn stop_service(&self) {
        call_void(Target::Service, "stopService");
    }

    fn request_focus(&self) {
        call_void(Target::Service, "requestFocus");
    }

    fn abandon_focus(&self) {
        call_void(Target::Service, "abandonFocus");
    }

    fn ui_changed(&self) {
        call_void(Target::Activity, "onUiChanged");
    }
}

// ---- Java -> Rust ---------------------------------------------------------------------

fn int_call<F: FnOnce() -> i32>(f: F) -> i32 {
    silence_panics();
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(0)
}
fn read_string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    env.get_string(value).ok().map(|s| s.into())
}


fn make_string(env: &JNIEnv, text: String) -> jstring {
    env.new_string(text).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeApiVersion(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    API_VERSION
}

/// Once per process, with the app's private files directory (where the last playback lives).
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeInit<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    dir: JString<'local>,
) -> i32 {
    int_call(|| {
        if let Some(path) = read_string(&env, &dir) {
            controller().init(&path);
        }
        0
    })
}

/// A button or list row was pressed. Returns effect flags for the Activity.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeClick(
    _env: JniEnv,
    _class: JniClass,
    id: i32,
) -> i32 {
    int_call(|| controller().click(id))
}

/// The whole screen as lines (see `ui.rs`). Null only if a panic was caught.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeScreen<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    silence_panics();
    let text = catch_unwind(AssertUnwindSafe(|| controller().screen())).unwrap_or_default();
    make_string(&env, text)
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeItem<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    index: i32,
) -> jstring {
    silence_panics();
    let text = catch_unwind(AssertUnwindSafe(|| controller().item(index.max(0) as usize)))
        .unwrap_or_default();
    make_string(&env, text)
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeItemCount(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    int_call(|| controller().item_count() as i32)
}

/// A command from the notification, the lock screen or a headset button.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeCommand(
    _env: JniEnv,
    _class: JniClass,
    command: i32,
) -> i32 {
    int_call(|| controller().command(command))
}

/// An audio focus change reported by Android (or 100: the request was refused).
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeFocus(
    _env: JniEnv,
    _class: JniClass,
    change: i32,
) -> i32 {
    int_call(|| {
        controller().on_focus(change);
        0
    })
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativePermissionResult(
    _env: JniEnv,
    _class: JniClass,
    granted: i32,
) -> i32 {
    int_call(|| {
        controller().permission_result(granted != 0);
        0
    })
}

/// Android launched BLACK WOOF with an audio URI (ACTION_VIEW). Returns effect flags.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeOpenExternal<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    uri: JString<'local>,
) -> i32 {
    int_call(|| match read_string(&env, &uri) {
        Some(uri) => controller().open_external(&uri),
        None => 0,
    })
}

/// The service started: Rust pushes the current picture. 1 if a track is active, else 0 and
/// the service ends itself.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeServiceReady(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    int_call(|| if controller().service_ready() { 1 } else { 0 })
}

/// Clean close: write down where playback is.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeSave(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    int_call(|| {
        controller().save_now();
        0
    })
}

/// Stops playback and waits for the playback thread. Call when the process is done with audio.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeShutdown(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    int_call(|| {
        controller().shutdown();
        0
    })
}
