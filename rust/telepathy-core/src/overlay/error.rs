use std::fmt::Display;
use widestring::error::ContainsNul;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub(crate) struct Error {
    _kind: ErrorKind,
}

#[derive(Debug)]
enum ErrorKind {
    #[cfg(windows)]
    Windows(windows::core::Error),
    ContainsNul,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self._kind {
            #[cfg(windows)]
            ErrorKind::Windows(error) => write!(f, "windows error: {:?}", error),
            ErrorKind::ContainsNul => write!(f, "string contains nul byte"),
        }
    }
}

impl From<ContainsNul<u16>> for Error {
    fn from(_: ContainsNul<u16>) -> Self {
        Error {
            _kind: ErrorKind::ContainsNul,
        }
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Error {
            _kind: ErrorKind::Windows(error),
        }
    }
}
