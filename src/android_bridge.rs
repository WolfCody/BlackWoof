//! JNI surface (Android only).
//!
//! Java -> Rust: every export takes and returns primitives only, and every body runs inside
//! `guarded`, so a panic never unwinds through the `extern "system"` boundary. Return codes:
//! 0 = success, otherwise an `ErrorKind` code.
//!
//! Rust -> Java: `JNI_OnLoad` caches the `JavaVM` and a global reference to `PlayerService`
//! (the app class loader is only reachable from there). The engine observer calls the static
//! `PlayerService.onEngineState(int)` on the playback thread. Rust decides when Android is
//! told anything: only final states, because only they end the service.
//!
//! Descriptor contract: Java passes a descriptor obtained with
//! `ParcelFileDescriptor.detachFd()`. `nativePlay` takes ownership on entry, whatever the
//! outcome, so Java must never close it afterwards.

// JNI export names are dictated by the JVM and cannot be snake_case.
#![allow(non_snake_case)]

use std::os::raw::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};

use jni::objects::{GlobalRef, JClass, JValue};
use jni::sys::{jint, JNI_ERR, JNI_VERSION_1_6};
use jni::JavaVM;

use crate::aaudio::AAudioSink;
use crate::audio_engine::{pack_state, Engine, Sink, State};
use crate::error::{Error, ErrorKind};
use crate::fd::take_fd;
use crate::guard::{guarded, silence_panics};

/// Increase when the Java <-> native contract changes.
const API_VERSION: i32 = 3;

const SERVICE_CLASS_NAME: &str = "com/dzekiwolf/blackwoof/PlayerService";
const SERVICE_CALLBACK: &str = "onEngineState";

type JniEnv = *mut c_void;
type JniClass = *mut c_void;

static ENGINE: OnceLock<Engine> = OnceLock::new();
static VM: OnceLock<JavaVM> = OnceLock::new();
static SERVICE_CLASS: OnceLock<GlobalRef> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let engine = Engine::new(Arc::new(|| -> Box<dyn Sink> { Box::new(AAudioSink::new()) }));
        engine.set_observer(Arc::new(notify_service));
        engine
    })
}

fn call<F: FnOnce() -> Result<(), Error>>(f: F) -> i32 {
    silence_panics();
    guarded(f)
}

/// Runs once when the library is loaded. Failing here makes `System.loadLibrary` throw, which
/// the Java side turns into "engine not loaded" instead of a silently stuck service.
#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: JavaVM, _reserved: *mut c_void) -> jint {
    silence_panics();
    let prepared = catch_unwind(AssertUnwindSafe(|| {
        let mut env = vm.get_env().ok()?;
        let class = env.find_class(SERVICE_CLASS_NAME).ok()?;
        env.new_global_ref(&class).ok()
    }));
    match prepared {
        Ok(Some(global)) => {
            let _ = SERVICE_CLASS.set(global);
            let _ = VM.set(vm);
            JNI_VERSION_1_6
        }
        _ => JNI_ERR,
    }
}

/// Runs on the playback thread, after the engine state is visible and with no lock held.
/// Never runs inside an audio write: the engine calls it between writes.
fn notify_service(state: State, error: Option<ErrorKind>) {
    if !state.is_final() {
        return;
    }
    let (Some(vm), Some(class)) = (VM.get(), SERVICE_CLASS.get()) else {
        return;
    };
    let packed = pack_state(state, error);
    if let Ok(mut env) = vm.attach_current_thread() {
        // SAFETY: the global reference keeps the class object alive; this wrapper does not
        // own it and deleting the wrapper does not release anything.
        let class = unsafe { JClass::from_raw(class.as_obj().as_raw()) };
        let _ = env.call_static_method(&class, SERVICE_CALLBACK, "(I)V", &[JValue::Int(packed)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeApiVersion(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    API_VERSION
}

/// `fd` must come from `ParcelFileDescriptor.detachFd()`. Ownership always moves to native.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativePlay(
    _env: JniEnv,
    _class: JniClass,
    fd: i32,
) -> i32 {
    call(move || {
        let file = take_fd(fd)?;
        engine().play(file)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativePause(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    call(|| {
        engine().pause();
        Ok(())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeResume(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    call(|| {
        engine().resume();
        Ok(())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeStop(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    call(|| {
        engine().stop();
        Ok(())
    })
}

/// Low 8 bits: `State` (0 idle, 1 opening, 2 playing, 3 paused, 4 ended, 5 stopped,
/// 6 failed). Bits 8..15: `ErrorKind` code after a failure. -1 if a panic was caught.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeState(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    silence_panics();
    match catch_unwind(AssertUnwindSafe(|| {
        let (state, error) = engine().state();
        pack_state(state, error)
    })) {
        Ok(packed) => packed,
        Err(_) => -1,
    }
}

/// 1 while a track is opening, playing or paused (the service has a reason to run), else 0.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeIsActive(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    silence_panics();
    match catch_unwind(AssertUnwindSafe(|| engine().state().0.is_active())) {
        Ok(true) => 1,
        _ => 0,
    }
}

/// Stops playback and waits for the playback thread. Call when the process is done with audio.
#[no_mangle]
pub extern "system" fn Java_com_dzekiwolf_blackwoof_Native_nativeShutdown(
    _env: JniEnv,
    _class: JniClass,
) -> i32 {
    call(|| {
        engine().shutdown();
        Ok(())
    })
}
