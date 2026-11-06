//! BLACK WOOF, Stage 2: audio engine and decoder.
//!
//! Policy for this crate: no logging, no diagnostic counters, nothing written to disk.
//! Errors are values (`Result<_, Error>`). A panic never crosses the JNI boundary.

pub mod audio_engine;
pub mod decoder;
pub mod error;
pub mod guard;

#[cfg(unix)]
pub mod fd;

#[cfg(target_os = "android")]
mod aaudio;
// `pub` so the `#[no_mangle]` JNI exports are certainly reachable from the cdylib.
#[cfg(target_os = "android")]
pub mod android_bridge;

#[cfg(test)]
mod testutil;
