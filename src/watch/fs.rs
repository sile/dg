use std;
use std::collections::HashMap;
use std::fs::DirEntry;
use std::path::{Path, PathBuf};
use fibers::{BoxSpawn, Spawn};
use fibers::sync::mpsc;
use fibers_tasque::{AsyncCall, DefaultIoTaskQueue, TaskQueueExt};
use futures::{Async, Future, Poll, Stream};
use futures::future::Fuse;
use inotify::{EventMask, WatchMask};

use {Error, ErrorKind, Result};
use inotify_service::{Event as InotifyEvent, InotifyService, InotifyServiceHandle, WatchHandle};
use watch::file::{FileEvent, WatchedFile};

#[derive(Debug)]
pub struct FileSystemWatcher {
    spawner: BoxSpawn,
    inotify: InotifyServiceHandle,
    dir_event_rx: mpsc::Receiver<DirectoryEvent>,
    dir_event_tx: mpsc::Sender<DirectoryEvent>,
    watched_files: HashMap<PathBuf, mpsc::Sender<FileEvent>>,
}
impl FileSystemWatcher {
    pub fn new<S>(spawner: S) -> Result<Self>
    where
        S: Spawn + Send + 'static,
    {
        let inotify = track!(InotifyService::start())?;
        let (dir_event_tx, dir_event_rx) = mpsc::channel();
        Ok(FileSystemWatcher {
            spawner: spawner.boxed(),
            inotify,
            dir_event_rx,
            dir_event_tx,
            watched_files: HashMap::new(),
        })
    }
    pub fn watch<P: AsRef<Path>>(&mut self, root_dir: P) -> Result<()> {
        let root_dir = root_dir.as_ref().to_path_buf();
        let watcher = track!(DirectoryWatcher::new(&self.inotify, &root_dir))?;
        println!("[DEBUG] watch: {:?}", root_dir);

        let tx0 = self.dir_event_tx.clone();
        let tx1 = tx0.clone();
        self.spawner.spawn(
            watcher
                .for_each(move |event| tx0.send(event).map_err(Error::from))
                .then(move |r| {
                    println!("TODO: result: {:?}", r);
                    let _ = tx1.send(DirectoryEvent::Removed {
                        path: root_dir,
                        is_dir: true,
                    });
                    Ok(())
                }),
        );
        Ok(())
    }
    fn handle_dir_event(&mut self, dir_event: DirectoryEvent) -> Option<WatchedFile> {
        match dir_event {
            DirectoryEvent::Updated { path, is_dir: true } => {
                let _ = self.watch(path);
                None
            }
            DirectoryEvent::Removed { is_dir: true, .. } => None,
            DirectoryEvent::Updated {
                path,
                is_dir: false,
            } => {
                if let Some(file_event_tx) = self.watched_files.get(&path).cloned() {
                    if file_event_tx.send(FileEvent::Updated).is_err() {
                        self.watched_files.remove(&path);
                    } else {
                        return None;
                    }
                }
                let (file_event_tx, file_event_rx) = mpsc::channel();
                self.watched_files.insert(path.clone(), file_event_tx);
                let file = WatchedFile::new(path, file_event_rx);
                Some(file)
            }
            DirectoryEvent::Removed {
                path,
                is_dir: false,
            } => {
                self.watched_files.remove(&path);
                None
            }
        }
    }
}
impl Stream for FileSystemWatcher {
    type Item = WatchedFile;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        while let Async::Ready(Some(dir_event)) = self.dir_event_rx.poll().expect("Never fails") {
            if let Some(file) = self.handle_dir_event(dir_event) {
                return Ok(Async::Ready(Some(file)));
            }
        }
        Ok(Async::NotReady)
    }
}

#[derive(Debug)]
struct DirectoryWatcher {
    path: PathBuf,
    watch: WatchHandle,
    list: ListDirectory,
}
impl DirectoryWatcher {
    fn new<P: AsRef<Path>>(inotify: &InotifyServiceHandle, path: P) -> Result<Self> {
        track_assert!(
            path.as_ref().is_dir(),
            ErrorKind::InvalidInput,
            "not directory: {:?}",
            path.as_ref()
        );
        let mask = WatchMask::CREATE | WatchMask::DELETE | WatchMask::DELETE_SELF
            | WatchMask::MODIFY | WatchMask::MOVE | WatchMask::MOVE_SELF;
        let watch = inotify.watch(&path, mask);
        Ok(DirectoryWatcher {
            path: path.as_ref().to_path_buf(),
            watch,
            list: ListDirectory::new(path),
        })
    }
    fn handle_inotify_event(&mut self, mut event: InotifyEvent) -> Option<Option<DirectoryEvent>> {
        if event
            .mask
            .intersects(EventMask::DELETE_SELF | EventMask::MOVE_SELF | EventMask::IGNORED)
        {
            None
        } else if event
            .mask
            .intersects(EventMask::CREATE | EventMask::MODIFY | EventMask::MOVED_TO)
        {
            let name = event.name.take().expect("Never fails");
            let path = self.path.join(name);
            let is_dir = event.mask.intersects(EventMask::ISDIR);
            Some(Some(DirectoryEvent::Updated { path, is_dir }))
        } else if event
            .mask
            .intersects(EventMask::DELETE | EventMask::MOVED_FROM)
        {
            let name = event.name.take().expect("Never fails");
            let path = self.path.join(name);
            let is_dir = event.mask.intersects(EventMask::ISDIR);
            Some(Some(DirectoryEvent::Removed { path, is_dir }))
        } else {
            println!("[TODO] Unknown event: {:?}", event);
            Some(None)
        }
    }
}
impl Stream for DirectoryWatcher {
    type Item = DirectoryEvent;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        while let Async::Ready(Some(entry)) = track!(self.list.poll())? {
            let path = entry.path();
            if let Ok(ft) = entry.file_type() {
                let is_dir = ft.is_dir();
                return Ok(Async::Ready(Some(DirectoryEvent::Updated { path, is_dir })));
            }
        }
        match track!(self.watch.poll())? {
            Async::NotReady => Ok(Async::NotReady),
            Async::Ready(None) => Ok(Async::Ready(None)),
            Async::Ready(Some(event)) => match self.handle_inotify_event(event) {
                None => Ok(Async::Ready(None)),
                Some(None) => self.poll(),
                Some(Some(dir_event)) => Ok(Async::Ready(Some(dir_event))),
            },
        }
    }
}

#[derive(Debug)]
struct ListDirectory {
    future: Fuse<AsyncCall<Result<Vec<DirEntry>>>>,
    entries: Vec<DirEntry>,
}
impl ListDirectory {
    fn new<P: AsRef<Path>>(dir: P) -> Self {
        let dir = dir.as_ref().to_path_buf();

        // TODO: starts listing after `dir` is added to inotify watches (for preventing entries loss)
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

#[derive(Debug)]
enum DirectoryEvent {
    Updated { path: PathBuf, is_dir: bool },
    Removed { path: PathBuf, is_dir: bool },
}
