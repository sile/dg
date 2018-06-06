use fibers::sync::mpsc;
use fibers::time::timer::{self, Timeout};
use fibers_tasque::{AsyncCall, DefaultIoTaskQueue, TaskQueueExt};
use futures::future::Fuse;
use futures::{Async, Future, Poll, Stream};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use {Error, Result};

const MIN_READ_CONTENT_INTERVAL: u64 = 60;
const READ_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug)]
pub struct FileUpdated;

#[derive(Debug)]
pub enum FileWatcher {
    Plain(PlainFileWatcher),
    Gzip,
    TarGzip,
}
impl FileWatcher {
    pub fn new<P: AsRef<Path>>(path: P, event_rx: mpsc::Receiver<FileUpdated>) -> Self {
        // TODO: guess file type
        // TODO: return Vec or Stream
        FileWatcher::Plain(PlainFileWatcher::new(path, event_rx))
    }
    pub fn path(&self) -> &Path {
        match *self {
            FileWatcher::Plain(ref w) => w.path(),
            FileWatcher::Gzip => unimplemented!(),
            FileWatcher::TarGzip => unimplemented!(),
        }
    }
}
impl Stream for FileWatcher {
    type Item = FileContent;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match *self {
            FileWatcher::Plain(ref mut w) => track!(w.poll()),
            FileWatcher::Gzip => unimplemented!(),
            FileWatcher::TarGzip => unimplemented!(),
        }
    }
}

#[derive(Debug)]
pub struct FileContent {
    pub offset: u64,
    pub data: Vec<u8>,
    pub eof: bool,
}

#[derive(Debug)]
pub struct PlainFileWatcher {
    path: PathBuf,
    event_rx: mpsc::Receiver<FileUpdated>,
    current_position: u64,
    read_file_content: Option<ReadFileContent>,
    is_updated: bool,
}
impl PlainFileWatcher {
    pub fn new<P: AsRef<Path>>(path: P, event_rx: mpsc::Receiver<FileUpdated>) -> Self {
        let mut this = PlainFileWatcher {
            path: path.as_ref().to_path_buf(),
            event_rx,
            current_position: 0,
            read_file_content: None,
            is_updated: false,
        };
        this.start_read_file_content(Duration::from_secs(0));
        this
    }
    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }
    fn start_read_file_content(&mut self, wait: Duration) {
        self.is_updated = false;
        self.read_file_content = Some(ReadFileContent::new(
            self.path.clone(),
            self.current_position,
            wait,
        ));
    }
}
impl Stream for PlainFileWatcher {
    type Item = FileContent;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.event_rx.poll().expect("Never fails") {
            Async::NotReady => {}
            Async::Ready(None) => return Ok(Async::Ready(None)),
            Async::Ready(Some(FileUpdated)) => self.is_updated = true,
        }
        if self.read_file_content.is_none() && self.is_updated {
            let wait = Duration::from_secs(MIN_READ_CONTENT_INTERVAL);
            self.start_read_file_content(wait);
        }
        if let Async::Ready(Some(content)) = track!(self.read_file_content.poll())? {
            self.read_file_content = None;
            self.current_position = content.offset + content.data.len() as u64;
            if !content.eof {
                self.start_read_file_content(Duration::from_secs(0));
            }
            if !content.data.is_empty() {
                return Ok(Async::Ready(Some(content)));
            }
        }
        Ok(Async::NotReady)
    }
}

#[derive(Debug)]
struct ReadFileContent {
    path: PathBuf,
    offset: u64,
    wait: Fuse<Timeout>,
    read: Option<AsyncCall<Result<FileContent>>>,
}
impl ReadFileContent {
    fn new(path: PathBuf, offset: u64, wait: Duration) -> Self {
        ReadFileContent {
            path,
            offset,
            wait: timer::timeout(wait).fuse(),
            read: None,
        }
    }
}
impl Future for ReadFileContent {
    type Item = FileContent;
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if let Async::Ready(()) = track!(self.wait.poll().map_err(Error::from))? {
            let path = self.path.clone();
            let offset = self.offset;
            let future = DefaultIoTaskQueue.async_call(move || {
                let mut file = track!(File::open(path).map_err(Error::from))?;
                track!(file.seek(SeekFrom::Start(offset)).map_err(Error::from))?;

                let mut buf = vec![0; READ_BUFFER_SIZE];
                let read_size = track!(file.read(&mut buf).map_err(Error::from))?;
                let eof = read_size < buf.len();
                buf.truncate(read_size);
                let content = FileContent {
                    offset,
                    data: buf,
                    eof,
                };
                Ok(content)
            });
            self.read = Some(future);
        }
        if let Async::Ready(Some(content)) = track!(self.read.poll().map_err(Error::from))? {
            let content = track!(content)?;
            Ok(Async::Ready(content))
        } else {
            Ok(Async::NotReady)
        }
    }
}
