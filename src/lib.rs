extern crate fibers;
extern crate fibers_inotify;
extern crate fibers_tasque;
extern crate futures;
extern crate inotify;
#[macro_use]
extern crate trackable;

pub use error::{Error, ErrorKind};

pub mod watch;

mod error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct AsyncResult<T>(fibers::sync::oneshot::Monitor<T, Error>);
impl<T> futures::Future for AsyncResult<T> {
    type Item = T;
    type Error = Error;
    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        track!(self.0.poll().map_err(Error::from))
    }
}
