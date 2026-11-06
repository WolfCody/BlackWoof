//! Error type. It carries a kind and nothing else: no text, no history.
//! BLACK WOOF keeps no logs; a kind is enough for the caller to show one short message.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorKind {
    /// The file descriptor is negative or not open.
    InvalidFd = 1,
    /// The file cannot be read.
    Io = 2,
    /// Not a recognised audio container.
    UnknownFormat = 3,
    /// The container holds no decodable audio.
    NoAudio = 4,
    /// Codec, sample rate or channel layout is not supported.
    Unsupported = 5,
    /// Too much damaged data to continue.
    Corrupt = 6,
    /// The audio output could not be opened.
    OutputOpen = 7,
    /// The audio output refused to start or pause.
    OutputControl = 8,
    /// Writing to the audio output failed.
    OutputWrite = 9,
    /// A panic was caught: this is a bug.
    Internal = 10,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
}

impl Error {
    pub const fn new(kind: ErrorKind) -> Error {
        Error { kind }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.kind)
    }
}

impl std::error::Error for Error {}
