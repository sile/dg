use std::path::Path;
use futures::{Async, Poll, Stream};
use inotify::WatchMask;

use {Error, Result};
use inotify_service::{Event as InotifyEvent, InotifyService, InotifyServiceHandle, WatchHandle};

#[derive(Debug)]
pub struct FileSystemWatcher {
    inotify: InotifyServiceHandle,
    watches: Vec<WatchHandle>,
}
impl FileSystemWatcher {
    pub fn new() -> Result<Self> {
        let inotify = track!(InotifyService::start())?;
        Ok(FileSystemWatcher {
            inotify,
            watches: Vec::new(),
        })
    }
    pub fn add_path<P: AsRef<Path>>(&mut self, path: P) {
        let mask = WatchMask::CREATE | WatchMask::DELETE | WatchMask::DELETE_SELF
            | WatchMask::MODIFY | WatchMask::MOVE | WatchMask::MOVE_SELF;
        let watch = self.inotify.watch(path, mask);
        self.watches.push(watch);
    }
}
impl Stream for FileSystemWatcher {
    type Item = InotifyEvent;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut i = 0;
        while i < self.watches.len() {
            match track_try_unwrap!(self.watches[i].poll()) {
                Async::NotReady => {
                    i += 1;
                }
                Async::Ready(None) => {
                    self.watches.swap_remove(i);
                }
                Async::Ready(Some(event)) => return Ok(Async::Ready(Some(event))),
            }
        }
        Ok(Async::NotReady)
    }
}
