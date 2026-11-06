//! The only place where a raw file descriptor becomes an owned `File`.
//!
//! Contract with the Java side: the descriptor comes from
//! `ParcelFileDescriptor.detachFd()`, so Java no longer owns it and never closes it.
//! `take_fd` runs on entry of every native function that receives a descriptor, so the
//! descriptor is closed exactly once on every path, including errors and panics.

use std::fs::File;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::raw::c_int;

use crate::error::{Error, ErrorKind};

extern "C" {
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
}

const F_GETFD: c_int = 1;

/// True if `fd` is an open descriptor. Has no side effects.
pub fn is_open(fd: i32) -> bool {
    if fd < 0 {
        return false;
    }
    // SAFETY: F_GETFD only queries the descriptor flags.
    let r = unsafe { fcntl(fd, F_GETFD) };
    r >= 0
}

/// Takes ownership of `fd`. On `Ok` the returned `File` is the only owner and closes the
/// descriptor when dropped. On `Err` nothing was taken and nothing is closed.
pub fn take_fd(fd: i32) -> Result<File, Error> {
    if !is_open(fd) {
        return Err(Error::new(ErrorKind::InvalidFd));
    }
    // SAFETY: `fd` is open (checked above) and, by contract, ownership is transferred to us.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    Ok(File::from(owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{lock_fd, open_temp};
    use std::os::fd::IntoRawFd;

    #[test]
    fn rejects_invalid_descriptors() {
        let _g = lock_fd();
        assert_eq!(take_fd(-1).unwrap_err().kind, ErrorKind::InvalidFd);
        assert_eq!(take_fd(1_000_000).unwrap_err().kind, ErrorKind::InvalidFd);
    }

    #[test]
    fn owns_and_closes_exactly_once() {
        let _g = lock_fd();
        let raw = open_temp(b"x").into_raw_fd();
        assert!(is_open(raw));
        let file = take_fd(raw).expect("valid descriptor");
        assert!(is_open(raw));
        drop(file);
        assert!(!is_open(raw), "descriptor must be closed when the File is dropped");
    }
}
