use fibers::sync::mpsc;
use fibers::{BoxSpawn, Spawn};
use fibers_inotify::InotifyService;
use futures::{Async, Future, Poll, Stream};
use slog::Logger;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use watch::fs::{DirectoryEvent, DirectoryWatcher};
use watch::fs::{FileUpdated, FileWatcher};
use {Error, Result};

#[derive(Debug)]
pub struct FileSystemWatcher {
    logger: Logger,
    spawner: BoxSpawn,
    inotify_service: InotifyService,
    dir_event_rx: mpsc::Receiver<DirectoryEvent>,
    dir_event_tx: mpsc::Sender<DirectoryEvent>,
    watching_files: HashMap<PathBuf, mpsc::Sender<FileUpdated>>,
}
impl FileSystemWatcher {
    pub fn new<S>(logger: Logger, spawner: S) -> Self
    where
        S: Spawn + Send + 'static,
    {
        info!(logger, "Creates FileSystemWatcher");
        let inotify_service = InotifyService::new();
        let (dir_event_tx, dir_event_rx) = mpsc::channel();
        FileSystemWatcher {
            logger,
            spawner: spawner.boxed(),
            inotify_service,
            dir_event_rx,
            dir_event_tx,
            watching_files: HashMap::new(),
        }
    }
    pub fn watch<P: AsRef<Path>>(&mut self, root_dir: P) -> Result<()> {
        let root_dir = root_dir.as_ref().to_path_buf();

        let logger = self.logger.new(o!("dir" => format!("{:?}", root_dir)));
        info!(logger, "Starts watching");

        let watcher = track!(DirectoryWatcher::new(
            logger.clone(),
            &self.inotify_service,
            &root_dir
        ))?;

        let tx = self.dir_event_tx.clone();
        let future = watcher
            .for_each(move |event| tx.send(event).map_err(Error::from))
            .then(move |result| {
                if let Err(e) = result {
                    error!(logger, "Watching terminated abnormally: \n{}", e);
                } else {
                    info!(logger, "Finished watching");
                }
                Ok(())
            });
        self.spawner.spawn(future);
        Ok(())
    }
    fn handle_dir_event(&mut self, dir_event: DirectoryEvent) -> Option<FileWatcher> {
        match dir_event {
            DirectoryEvent::Updated { path, is_dir: true } => {
                if let Err(e) = self.watch(&path) {
                    error!(
                        self.logger,
                        "Cannot start watching the directory: {:?}\n{}", path, e
                    );
                }
                None
            }
            DirectoryEvent::Removed { is_dir: true, .. } => None,
            DirectoryEvent::Updated {
                path,
                is_dir: false,
            } => {
                if let Some(file_event_tx) = self.watching_files.get(&path).cloned() {
                    debug!(self.logger, "The file {:?} is updated", path);
                    if file_event_tx.send(FileUpdated).is_err() {
                        self.watching_files.remove(&path);
                    } else {
                        return None;
                    }
                }

                debug!(self.logger, "Starts watching file: {:?}", path);
                let (file_event_tx, file_event_rx) = mpsc::channel();
                self.watching_files.insert(path.clone(), file_event_tx);
                let file = FileWatcher::new(path, file_event_rx);
                Some(file)
            }
            DirectoryEvent::Removed {
                path,
                is_dir: false,
            } => {
                debug!(self.logger, "Stops watching file: {:?}", path);
                self.watching_files.remove(&path);
                None
            }
        }
    }
}
impl Stream for FileSystemWatcher {
    type Item = FileWatcher;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        track!(self.inotify_service.poll().map_err(Error::from))?;
        while let Async::Ready(Some(dir_event)) = self.dir_event_rx.poll().expect("Never fails") {
            if let Some(file) = self.handle_dir_event(dir_event) {
                return Ok(Async::Ready(Some(file)));
            }
        }
        Ok(Async::NotReady)
    }
}
