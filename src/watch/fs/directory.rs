use fibers_inotify::{EventMask, InotifyEvent, InotifyService, WatchMask, Watcher, WatcherEvent};
use fibers_tasque::{AsyncCall, DefaultIoTaskQueue, TaskQueueExt};
use futures::future::Fuse;
use futures::{Async, Future, Poll, Stream};
use std;
use std::fs::DirEntry;
use std::path::{Path, PathBuf};

use {Error, ErrorKind, Result};

#[derive(Debug)]
pub struct DirectoryWatcher {
    path: PathBuf,
    watcher: Watcher,
    list_dir: Option<ListDirectory>,
}
impl DirectoryWatcher {
    pub fn new<P: AsRef<Path>>(inotify: &InotifyService, path: P) -> Result<Self> {
        track_assert!(
            path.as_ref().is_dir(),
            ErrorKind::InvalidInput,
            "not a directory: {:?}",
            path.as_ref()
        );
        let mask = WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::DELETE_SELF
            | WatchMask::MODIFY
            | WatchMask::MOVE
            | WatchMask::MOVE_SELF
            | WatchMask::EXCL_UNLINK;
        let watcher = inotify.handle().watch(&path, mask);
        Ok(DirectoryWatcher {
            path: path.as_ref().to_path_buf(),
            watcher,
            list_dir: None,
        })
    }
    fn poll_directory_listing(&mut self) -> Result<Option<DirectoryEvent>> {
        if let Some(mut list_dir) = self.list_dir.take() {
            match track!(list_dir.poll())? {
                Async::NotReady => {
                    self.list_dir = Some(list_dir);
                    Ok(None)
                }
                Async::Ready(None) => Ok(None),
                Async::Ready(Some(entry)) => {
                    self.list_dir = Some(list_dir);
                    let path = entry.path();
                    let is_dir = entry.file_type().ok().map_or(false, |ft| ft.is_dir());
                    Ok(Some(DirectoryEvent::Updated { path, is_dir }))
                }
            }
        } else {
            Ok(None)
        }
    }
    fn poll_watcher(&mut self) -> Result<Action> {
        match track!(self.watcher.poll())? {
            Async::NotReady => Ok(Action::Wait),
            Async::Ready(None) => Ok(Action::Terminate),
            Async::Ready(Some(watcher_event)) => {
                let action = match watcher_event {
                    WatcherEvent::Notified(inotify_event) => {
                        self.handle_inotify_event(inotify_event)
                    }
                    WatcherEvent::StartWatching => {
                        self.list_dir = Some(ListDirectory::new(self.path.clone()));
                        Action::Continue
                    }
                    WatcherEvent::RestartWatching => Action::Terminate,
                };
                Ok(action)
            }
        }
    }
    fn handle_inotify_event(&mut self, mut event: InotifyEvent) -> Action {
        if event
            .mask
            .intersects(EventMask::DELETE_SELF | EventMask::MOVE_SELF | EventMask::IGNORED)
        {
            Action::Terminate
        } else if event
            .mask
            .intersects(EventMask::CREATE | EventMask::MODIFY | EventMask::MOVED_TO)
        {
            let name = event.name.take().expect("Never fails");
            let path = self.path.join(name);
            let is_dir = event.mask.intersects(EventMask::ISDIR);
            Action::Notify(DirectoryEvent::Updated { path, is_dir })
        } else if event
            .mask
            .intersects(EventMask::DELETE | EventMask::MOVED_FROM)
        {
            let name = event.name.take().expect("Never fails");
            let path = self.path.join(name);
            let is_dir = event.mask.intersects(EventMask::ISDIR);
            Action::Notify(DirectoryEvent::Removed { path, is_dir })
        } else {
            Action::Continue
        }
    }
}
impl Stream for DirectoryWatcher {
    type Item = DirectoryEvent;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            if let Some(event) = track!(self.poll_directory_listing())? {
                return Ok(Async::Ready(Some(event)));
            }
            match track!(self.poll_watcher())? {
                Action::Continue => {}
                Action::Wait => return Ok(Async::NotReady),
                Action::Terminate => return Ok(Async::Ready(None)),
                Action::Notify(event) => return Ok(Async::Ready(Some(event))),
            }
        }
    }
}

#[derive(Debug)]
pub enum DirectoryEvent {
    Updated { path: PathBuf, is_dir: bool },
    Removed { path: PathBuf, is_dir: bool },
}

#[derive(Debug)]
enum Action {
    Terminate,
    Wait,
    Continue,
    Notify(DirectoryEvent),
}

#[derive(Debug)]
struct ListDirectory {
    future: Fuse<AsyncCall<Result<Vec<DirEntry>>>>,
    entries: Vec<DirEntry>,
}
impl ListDirectory {
    fn new(dir: PathBuf) -> Self {
        let future = DefaultIoTaskQueue.async_call(move || {
            let mut entries = Vec::new();
            for entry in track!(std::fs::read_dir(dir).map_err(Error::from))? {
                let entry = track!(entry.map_err(Error::from))?;
                entries.push(entry);
            }
            Ok(entries)
        });
        ListDirectory {
            future: future.fuse(),
            entries: Vec::new(),
        }
    }
}
impl Stream for ListDirectory {
    type Item = DirEntry;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if let Async::Ready(entries) = track!(self.future.poll().map_err(Error::from))? {
            self.entries = track!(entries)?;
        }
        if let Some(entry) = self.entries.pop() {
            Ok(Async::Ready(Some(entry)))
        } else {
            Ok(Async::NotReady)
        }
    }
}
