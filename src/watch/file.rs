use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;
use fibers::sync::mpsc;
use fibers::time::timer::{self, Timeout};
use futures::{Async, Future, Poll, Stream};

use {Error, Result};

#[derive(Debug, Clone)]
pub enum FileEvent {
    Updated,
}

#[derive(Debug)]
pub enum WatchedFile {
    Text(WatchedTextFile),
    // BinaryFile(BinaryFile),
    // TODO: gz, tar.gz
}
impl WatchedFile {
    pub fn new<P: AsRef<Path>>(path: P, event_rx: mpsc::Receiver<FileEvent>) -> Self {
        // TODO: guess
        WatchedFile::Text(WatchedTextFile::new(path, event_rx))
    }

    pub fn path(&self) -> PathBuf {
        match *self {
            WatchedFile::Text(ref f) => f.path.clone(),
        }
    }

    pub fn subscribe(&mut self) -> FileSubscriber {
        match *self {
            WatchedFile::Text(ref mut f) => FileSubscriber::Text(f.subscribe()),
        }
    }
}
impl Future for WatchedFile {
    type Item = ();
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match *self {
            WatchedFile::Text(ref mut f) => track!(f.poll()),
        }
    }
}
impl From<WatchedTextFile> for WatchedFile {
    fn from(f: WatchedTextFile) -> Self {
        WatchedFile::Text(f)
    }
}

#[derive(Debug)]
pub enum FileSubscriber {
    Text(TextFileSubscriber),
}
impl Stream for FileSubscriber {
    type Item = FileChunk;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match *self {
            FileSubscriber::Text(ref mut f) => track!(f.poll()),
        }
    }
}

#[derive(Debug)]
pub struct WatchedTextFile {
    path: PathBuf,
    event_rx: mpsc::Receiver<FileEvent>,
    forwards: Vec<mpsc::Sender<FileEvent>>,
}
impl WatchedTextFile {
    pub fn new<P: AsRef<Path>>(path: P, event_rx: mpsc::Receiver<FileEvent>) -> Self {
        WatchedTextFile {
            path: path.as_ref().to_path_buf(),
            event_rx,
            forwards: Vec::new(),
        }
    }
    pub fn subscribe(&mut self) -> TextFileSubscriber {
        let (event_tx, event_rx) = mpsc::channel();
        self.forwards.push(event_tx);
        TextFileSubscriber {
            path: self.path.clone(),
            event_rx,
            position: 0,
            check_content: Some(timer::timeout(Duration::from_secs(1))),
            is_updated: false,
        }
    }
}
impl Future for WatchedTextFile {
    type Item = ();
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        while let Async::Ready(event) = self.event_rx.poll().expect("Never fails") {
            if let Some(event) = event {
                let mut i = 0;
                while i < self.forwards.len() {
                    if self.forwards[i].send(event.clone()).is_err() {
                        self.forwards.swap_remove(i);
                    } else {
                        i += 1;
                    }
                }
            } else {
                return Ok(Async::Ready(()));
            }
        }
        Ok(Async::NotReady)
    }
}

#[derive(Debug)]
pub struct TextFileSubscriber {
    path: PathBuf,
    event_rx: mpsc::Receiver<FileEvent>,
    position: u64,
    check_content: Option<Timeout>,
    is_updated: bool,
}
impl TextFileSubscriber {
    fn check_content(&mut self) -> Result<FileChunk> {
        // TODO: Use `TaskQueueExt::async_call`
        let mut file = track!(File::open(&self.path).map_err(Error::from))?;
        let mut buf = vec![0; 1024 * 1024]; // TODO
        track!(
            file.seek(SeekFrom::Start(self.position))
                .map_err(Error::from)
        )?;
        let mut read_size = track!(file.read(&mut buf).map_err(Error::from))?;
        while read_size > 0 {
            if buf[read_size - 1] < 0x80 {
                break;
            } else if (buf[read_size - 1] & 0b0100_0000) != 0 {
                read_size -= 1;
                break;
            } else {
                read_size -= 1;
            }
        }
        buf.truncate(read_size);

        let chunk = FileChunk {
            offset: self.position,
            data: track!(String::from_utf8(buf).map_err(Error::from))?,
        };
        self.position += read_size as u64;
        Ok(chunk)
    }
}
impl Stream for TextFileSubscriber {
    type Item = FileChunk;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.event_rx.poll().expect("Never fails") {
            Async::NotReady => {}
            Async::Ready(None) => return Ok(Async::Ready(None)),
            Async::Ready(Some(event)) => match event {
                FileEvent::Updated => {
                    self.is_updated = true;
                }
            },
        }
        if self.check_content.is_none() && self.is_updated {
            self.is_updated = false;
            self.check_content = Some(timer::timeout(Duration::from_secs(60))); // TODO
        }
        if let Async::Ready(Some(())) = track!(self.check_content.poll().map_err(Error::from))? {
            let chunk = track!(self.check_content())?;
            self.check_content = None;
            Ok(Async::Ready(Some(chunk)))
        } else {
            Ok(Async::NotReady)
        }
    }
}

#[derive(Debug)]
pub struct FileChunk {
    pub offset: u64,
    pub data: String,
}
