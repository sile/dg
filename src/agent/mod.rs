use std::collections::HashMap;
use std::path::PathBuf;
use fibers::{BoxSpawn, Spawn};
use fibers::sync::mpsc;
use futures::{Async, Future, Poll, Stream};
use rand::StdRng;
use scalable_cuckoo_filter::{DefaultHasher, ScalableCuckooFilter, ScalableCuckooFilterBuilder};
use slog::Logger;

use Error;
use tokenize::WordTokenizer;
use watch::fs::{FileContent, FileSystemWatcher};

#[derive(Debug)]
pub struct Agent {
    logger: Logger,
    spawner: BoxSpawn,
    fs_watcher: FileSystemWatcher,
    files: HashMap<PathBuf, FileState>,
    file_event_tx: mpsc::Sender<FileEvent>,
    file_event_rx: mpsc::Receiver<FileEvent>,
}
impl Agent {
    pub fn new<S>(logger: Logger, spawner: S, fs_watcher: FileSystemWatcher) -> Self
    where
        S: Spawn + Send + 'static,
    {
        info!(logger, "Starts agent");
        let (file_event_tx, file_event_rx) = mpsc::channel();
        Agent {
            logger,
            spawner: spawner.boxed(),
            fs_watcher,
            files: HashMap::new(),
            file_event_tx,
            file_event_rx,
        }
    }
    fn handle_file_event(&mut self, event: FileEvent) {
        match event {
            FileEvent::Updated { path, content } => self.handle_file_updated(path, content),
            FileEvent::Deleted { path } => self.handle_file_deleted(path),
        }
    }
    fn handle_file_deleted(&mut self, path: PathBuf) {
        self.files.remove(&path);
    }
    fn handle_file_updated(&mut self, path: PathBuf, content: FileContent) {
        if let Some(file) = self.files.get_mut(&path) {
            let offset = content.offset;
            let eof = content.eof;
            file.update_cuckoo_filter(content);
            info!(
                self.logger,
                "Cuckoo filter updated: path={:?}, offset={}, eof={}, filter=[bytes:{}, items:{}, cap:{}, bin:{}]",
                path,
                offset,
                eof,
                file.cuckoo_filter.bits() / 8,
                file.cuckoo_filter.len(),
                file.cuckoo_filter.capacity(),
                file.is_binary
            );
        }
    }
}
impl Future for Agent {
    type Item = ();
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        while let Async::Ready(Some(file_watcher)) = track!(self.fs_watcher.poll())? {
            let logger = self.logger
                .new(o!("path" => format!("{:?}", file_watcher.path())));
            info!(logger, "Starts file watching",);
            self.files
                .insert(file_watcher.path().to_path_buf(), FileState::new());

            let path0 = file_watcher.path().to_path_buf();
            let path1 = path0.clone();
            let file_event_tx0 = self.file_event_tx.clone();
            let file_event_tx1 = self.file_event_tx.clone();
            self.spawner.spawn(
                file_watcher
                    .for_each(move |content| {
                        let result = file_event_tx0.send(FileEvent::Updated {
                            path: path0.clone(),
                            content,
                        });
                        track!(result.map_err(Error::from))
                    })
                    .then(move |result| {
                        info!(logger, "Stops file watching: succeeded={}", result.is_ok());
                        let _ = file_event_tx1.send(FileEvent::Deleted { path: path1 });
                        Ok(())
                    }),
            );
        }
        while let Async::Ready(Some(file_event)) = self.file_event_rx.poll().expect("Never fails") {
            self.handle_file_event(file_event);
        }
        Ok(Async::NotReady)
    }
}

#[derive(Debug)]
enum FileEvent {
    Updated { path: PathBuf, content: FileContent },
    Deleted { path: PathBuf },
}

#[derive(Debug)]
struct FileState {
    cuckoo_filter: ScalableCuckooFilter<str, DefaultHasher, StdRng>,
    buf: Vec<u8>,
    is_binary: bool,
}
impl FileState {
    fn new() -> Self {
        let cuckoo_filter = ScalableCuckooFilterBuilder::new()
            .initial_capacity(100_000)
            .false_positive_probability(0.001)
            .rng(StdRng::new().unwrap())
            .finish();
        FileState {
            cuckoo_filter,
            buf: Vec::new(),
            is_binary: false,
        }
    }
    fn update_cuckoo_filter(&mut self, content: FileContent) {
        if self.is_binary {
            return;
        }

        self.buf.extend(content.data);
        let mut end = 0;
        for w in WordTokenizer::new(&self.buf) {
            match w {
                Err(_) => {
                    self.is_binary = true;
                    end = self.buf.len();
                    break;
                }
                Ok((start, w)) => {
                    self.cuckoo_filter.insert(w);
                    end = start + w.len();
                }
            }
        }
        if self.is_binary {
            self.buf.clear()
        } else {
            for _ in self.buf.drain(0..end) {}
        }
    }
}
