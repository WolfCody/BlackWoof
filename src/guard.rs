//! Panic containment. A Rust panic must never unwind through the JNI boundary.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Once;

use crate::error::{Error, ErrorKind};

/// Runs `f` and turns every outcome into a plain status code:
/// 0 = success, otherwise an `ErrorKind` code. A panic becomes `ErrorKind::Internal`.
pub fn guarded<F: FnOnce() -> Result<(), Error>>(f: F) -> i32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => e.kind as i32,
        Err(_) => ErrorKind::Internal as i32,
    }
}

/// Replaces the default panic hook with one that discards the message:
/// BLACK WOOF writes no logs. Used by the Android entry points only.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn silence_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::panic::set_hook(Box::new(|_| {})));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_results_and_contains_panics() {
        assert_eq!(guarded(|| Ok(())), 0);
        assert_eq!(guarded(|| Err(Error::new(ErrorKind::Io))), ErrorKind::Io as i32);
        let code = guarded(|| -> Result<(), Error> { panic!("deliberate test panic") });
        assert_eq!(code, ErrorKind::Internal as i32);
    }
}
