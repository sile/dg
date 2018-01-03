use std::io;
use std::sync::mpsc::RecvError;
use fibers::sync::oneshot::MonitorError;
use trackable::error::TrackableError;
use trackable::error::{ErrorKind as TrackableErrorKind, ErrorKindExt};

/// This crate specific error type.
#[derive(Debug, Clone)]
pub struct Error(TrackableError<ErrorKind>);
derive_traits_for_trackable_error_newtype!(Error, ErrorKind);
impl From<io::Error> for Error {
    fn from(f: io::Error) -> Self {
        ErrorKind::Other.cause(f).into()
    }
}
impl From<MonitorError<Error>> for Error {
    fn from(f: MonitorError<Error>) -> Self {
        f.unwrap_or_else(|| {
            ErrorKind::Other
                .cause("Monitor channel disconnected")
                .into()
        })
    }
}
impl From<RecvError> for Error {
    fn from(f: RecvError) -> Self {
        ErrorKind::Other.cause(f).into()
    }
}

/// The list of the possible error kinds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Other,
}
impl TrackableErrorKind for ErrorKind {}
