extern crate fibers;
extern crate fibers_inotify;
extern crate fibers_tasque;
extern crate futures;
extern crate rand;
extern crate scalable_cuckoo_filter;
#[macro_use]
extern crate slog;
#[macro_use]
extern crate trackable;

pub use error::{Error, ErrorKind};

pub mod agent;
pub mod watch;

mod error;

pub type Result<T> = std::result::Result<T, Error>;
