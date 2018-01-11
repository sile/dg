use std::collections::HashMap;
use std::path::{Path, PathBuf};
use fibers::{BoxSpawn, Spawn};
use fibers::sync::mpsc;
use fibers_inotify::InotifyService;
use futures::{Async, Future, Poll, Stream};
use slog::Logger;

use {Error, Result};
use watch::file::{FileEvent, WatchedFile};
use self::directory::{DirectoryEvent, DirectoryWatcher};

mod directory;

#[derive(Debug)]
pub struct FileSystemWatcher {
    logger: Logger,
    spawner: BoxSpawn,
    inotify_service: InotifyService,
    dir_event_rx: mpsc::Receiver<DirectoryEvent>,
    dir_event_tx: mpsc::Sender<DirectoryEvent>,
    watched_files: HashMap<PathBuf, mpsc::Sender<FileEvent>>,
}
impl FileSystemWatcher {
    pub fn new<S>(logger: Logger, spawner: S) -> Self
    where
        S: Spawn + Send + 'static,
    {
        let inotify_service = InotifyService::new();
        let (dir_event_tx, dir_event_rx) = mpsc::channel();
        FileSystemWatcher {
            logger,
            spawner: spawner.boxed(),
            inotify_service,
            dir_event_rx,
            dir_event_tx,
            watched_files: HashMap::new(),
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

        let tx0 = self.dir_event_tx.clone();
        let tx1 = tx0.clone();
        self.spawner.spawn(
            watcher
                .for_each(move |event| tx0.send(event).map_err(Error::from))
                .then(move |result| {
                    if let Err(e) = result {
                        error!(logger, "Finished watching: \n{}", e);
                    } else {
                        info!(logger, "Finished watching");
                    }
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
        track!(self.inotify_service.poll().map_err(Error::from))?;
        while let Async::Ready(Some(dir_event)) = self.dir_event_rx.poll().expect("Never fails") {
            if let Some(file) = self.handle_dir_event(dir_event) {
                return Ok(Async::Ready(Some(file)));
            }
        }
        Ok(Async::NotReady)
    }
}
