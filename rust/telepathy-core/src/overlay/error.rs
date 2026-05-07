#![allow(dead_code)]

use std::fmt::Display;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub(crate) struct Error {
    kind: ErrorKind,
}

#[derive(Debug)]
enum ErrorKind {
    #[cfg(windows)]
    Windows(windows::core::Error),
    #[cfg(windows)]
    ContainsNul,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            #[cfg(windows)]
            ErrorKind::Windows(error) => write!(f, "windows error: {:?}", error),
            #[cfg(windows)]
            ErrorKind::ContainsNul => write!(f, "string contains nul byte"),
            _ => write!(f, "unknown error"),
        }
    }
}

#[cfg(windows)]
impl From<widestring::error::ContainsNul<u16>> for Error {
    fn from(_: widestring::error::ContainsNul<u16>) -> Self {
        Error {
            kind: ErrorKind::ContainsNul,
        }
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Error {
            kind: ErrorKind::Windows(error),
        }
    }
}
