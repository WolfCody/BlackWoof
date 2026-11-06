//! JNI surface (Android only).
//!
//! Every export takes and returns primitives only, so no JNI object handling is needed in
//! Stage 2, and every body runs inside `guarded`: a panic never unwinds through the
//! `extern "system"` boundary. Return codes: 0 = success, otherwise an `ErrorKind` code.
//!
//! Descriptor contract: Java passes a descriptor obtained with
//! `ParcelFileDescriptor.detachFd()`. `nativePlay` takes ownership on entry, whatever the
//! outcome, so Java must never close it afterwards.

// JNI export names are dictated by the JVM and cannot be snake_case.
#![allow(non_snake_case)]

use std::os::raw::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};

use crate::aaudio::AAudioSink;
use crate::audio_engine::{Engine, Sink};
use crate::error::Error;
use crate::fd::take_fd;
use crate::guard::{guarded, silence_panics};

/// Increase when the Java <-> native contract changes.
const API_VERSION: i32 = 2;

type JniEnv = *mut c_void;
type JniClass = *mut c_void;

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        Engine::new(Arc::new(|| -> Box<dyn Sink> { Box::new(AAudioSink::new()) }))
    })
}

fn call<F: FnOnce() -> Result<(), Error>>(f: F) -> i32 {
    silence_panics();
    guarded(f)
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
        (state as i32) | (error.map_or(0, |kind| kind as i32) << 8)
    })) {
        Ok(packed) => packed,
        Err(_) => -1,
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
