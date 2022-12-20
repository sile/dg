use fibers::sync::oneshot::MonitorError;
use fibers_inotify;
use fibers_tasque;
use std;
use std::io;
use std::sync::mpsc::{RecvError, SendError};
use trackable::error::TrackableError;
use trackable::error::{ErrorKind as TrackableErrorKind, ErrorKindExt};

/// This crate specific error type.
#[derive(Debug, Clone, trackable::TrackableError)]
pub struct Error(TrackableError<ErrorKind>);
impl From<io::Error> for Error {
    fn from(f: io::Error) -> Self {
        ErrorKind::Other.cause(f).into()
    }
}
impl From<std::string::FromUtf8Error> for Error {
    fn from(f: std::string::FromUtf8Error) -> Self {
        ErrorKind::InvalidInput.cause(f).into()
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
impl From<std::str::Utf8Error> for Error {
    fn from(f: std::str::Utf8Error) -> Self {
        ErrorKind::InvalidInput.cause(f).into()
    }
}
impl<T: Send + Sync + 'static> From<SendError<T>> for Error {
    fn from(f: SendError<T>) -> Self {
        ErrorKind::Other.cause(f).into()
    }
}
impl From<fibers_tasque::AsyncCallError> for Error {
    fn from(f: fibers_tasque::AsyncCallError) -> Self {
        ErrorKind::Other.cause(f).into()
    }
}
impl From<fibers_inotify::Error> for Error {
    fn from(f: fibers_inotify::Error) -> Self {
        let kind = match *f.kind() {
            fibers_inotify::ErrorKind::InvalidInput => ErrorKind::InvalidInput,
            fibers_inotify::ErrorKind::ResourceShortage | fibers_inotify::ErrorKind::Other => {
                ErrorKind::Other
            }
        };
        kind.cause(f).into()
    }
}

/// The list of the possible error kinds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Other,
}
impl TrackableErrorKind for ErrorKind {}
