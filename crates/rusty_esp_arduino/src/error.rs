//! Why the last sketch call returned `false` or `None`.
//!
//! Arduino functions do not return errors; a maker checks a boolean or an
//! `Option`. The reason is still recorded, once, here — a sketch that prints
//! [`last_error`] when `cam::grab()` returns `None` sees "camera: begin was
//! not called" instead of guessing.

use std::fmt;
use std::sync::{Mutex, PoisonError};

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No board has been installed; see `board::install`.
    NoBoard,
    /// `begin` has not been called on the named peripheral.
    NotBegun(&'static str),
    /// A function package refused; the Janus error names why.
    Core(rusty_esp_core::error::Error),
    /// The operating system refused (a socket, a file); the text is its own.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoBoard => f.write_str("no board installed: call board::install first"),
            Error::NotBegun(what) => write!(f, "{what}: begin was not called"),
            Error::Core(e) => write!(f, "{e:?}"),
            Error::Io(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusty_esp_core::error::Error> for Error {
    fn from(e: rusty_esp_core::error::Error) -> Self {
        Error::Core(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

/// The facade's result type; sketch functions fold it into `bool` / `Option`.
pub type Result<T> = std::result::Result<T, Error>;

static LAST: Mutex<Option<Error>> = Mutex::new(None);

/// The reason for the most recent `false` or `None`, if any; cleared by the
/// next successful call.
pub fn last_error() -> Option<Error> {
    LAST.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

pub(crate) fn record(e: Error) {
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
}

pub(crate) fn clear() {
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Fold a result into a boolean, recording the error.
pub(crate) fn ok(r: Result<()>) -> bool {
    match r {
        Ok(()) => {
            clear();
            true
        }
        Err(e) => {
            record(e);
            false
        }
    }
}

/// Fold a result into an option, recording the error.
pub(crate) fn some<T>(r: Result<Option<T>>) -> Option<T> {
    match r {
        Ok(v) => {
            clear();
            v
        }
        Err(e) => {
            record(e);
            None
        }
    }
}
