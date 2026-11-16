//! BLACK WOOF: audio engine, queue, saved state and text screen, all in Rust.
//!
//! Policy for this crate: no logging, no diagnostic counters, and the only thing written to
//! disk is the last playback (`storage`). Errors are values (`Result<_, Error>`). A panic never
//! crosses the JNI boundary.

pub mod audio_engine;
pub mod decoder;
pub mod error;
pub mod guard;
pub mod media_session;
pub mod playlist;
pub mod storage;
pub mod ui;

#[cfg(unix)]
pub mod fd;
#[cfg(unix)]
pub mod playback_controller;

#[cfg(target_os = "android")]
mod aaudio;
// `pub` so the `#[no_mangle]` JNI exports are certainly reachable from the cdylib.
#[cfg(target_os = "android")]
pub mod android_bridge;

#[cfg(test)]
mod testutil;
