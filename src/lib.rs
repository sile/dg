extern crate fibers;
extern crate futures;
extern crate inotify;
#[macro_use]
extern crate trackable;

pub use error::{Error, ErrorKind};

pub mod watch;

mod error;

pub type Result<T> = std::result::Result<T, Error>;
