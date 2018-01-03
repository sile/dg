use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;
use fibers::sync::mpsc;
use futures::{Async, Poll, Stream};
use inotify::{Event, EventMask, Inotify, WatchMask};

use {Error, Result};

#[derive(Debug)]
pub struct OwnedEvent {
    pub mask: EventMask,
    pub cookie: u32,
    pub name: Option<OsString>,
}
impl<'a> From<Event<'a>> for OwnedEvent {
    fn from(f: Event<'a>) -> Self {
        OwnedEvent {
            mask: f.mask.clone(),
            cookie: f.cookie,
            name: f.name.map(|n| n.to_owned()),
        }
    }
}

#[derive(Debug)]
pub struct FileSystemWatcher {
    command_tx: std_mpsc::Sender<Command>,
    event_rx: mpsc::Receiver<Result<OwnedEvent>>,
}
impl FileSystemWatcher {
    pub fn new() -> Result<Self> {
        let (init_tx, init_rx) = std_mpsc::channel();
        let (command_tx, command_rx) = std_mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || match track!(Inotify::init().map_err(Error::from)) {
            Err(e) => {
                let _ = init_tx.send(Err(e));
            }
            Ok(inotify) => {
                let _ = init_tx.send(Ok(()));
                let mut watcher = FileSystemWatcherThread {
                    inotify,
                    command_rx,
                    event_tx,
                };
                loop {
                    match watcher.run_once() {
                        Err(e) => {
                            let _ = watcher.event_tx.send(track!(Err(e)));
                            break;
                        }
                        Ok(false) => break,
                        Ok(true) => {}
                    }
                }
            }
        });
        track!(init_rx.recv().map_err(Error::from).and_then(|r| r))?;
        Ok(FileSystemWatcher {
            command_tx,
            event_rx,
        })
    }
    pub fn add_path<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let (tx, rx) = std_mpsc::channel();
        let command = Command::Add {
            path: path.as_ref().to_path_buf(),
            reply: tx,
        };
        let _ = self.command_tx.send(command);
        track!(rx.recv().map_err(Error::from).and_then(|r| r))?;
        Ok(())
    }
}
impl Stream for FileSystemWatcher {
    type Item = OwnedEvent;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.event_rx.poll().expect("Never fails") {
            Async::NotReady => Ok(Async::NotReady),
            Async::Ready(None) => Ok(Async::Ready(None)),
            Async::Ready(Some(Err(e))) => Err(track!(e)),
            Async::Ready(Some(Ok(x))) => Ok(Async::Ready(Some(x))),
        }
    }
}

#[derive(Debug)]
enum Command {
    Add {
        path: PathBuf,
        reply: std_mpsc::Sender<Result<()>>,
    },
    // Remove(PathBuf),
}

struct FileSystemWatcherThread {
    inotify: Inotify,
    command_rx: std_mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<Result<OwnedEvent>>,
}
impl FileSystemWatcherThread {
    fn add_path<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let mask = WatchMask::CREATE | WatchMask::DELETE | WatchMask::DELETE_SELF
            | WatchMask::MODIFY | WatchMask::MOVE | WatchMask::MOVE_SELF;
        track!(self.inotify.add_watch(path, mask).map_err(Error::from))?;
        Ok(())
    }
    fn run_once(&mut self) -> Result<bool> {
        let mut buf = [0; 4096];
        let mut event_received = false;
        for event in track!(self.inotify.read_events(&mut buf).map_err(Error::from))? {
            event_received = true;
            self.handle_event(event);
        }
        if event_received {
            return Ok(true);
        }

        match self.command_rx.recv_timeout(Duration::from_millis(10)) {
            Err(RecvTimeoutError::Timeout) => Ok(true),
            Err(RecvTimeoutError::Disconnected) => Ok(false),
            Ok(command) => {
                self.handle_command(command);
                Ok(true)
            }
        }
    }
    fn handle_event(&mut self, event: Event) {
        let _ = self.event_tx.send(Ok(OwnedEvent::from(event)));
    }
    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Add { path, reply } => {
                let _ = reply.send(track!(self.add_path(path)));
            }
        }
    }
}
