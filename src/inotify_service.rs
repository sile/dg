use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;
use fibers::sync::{mpsc, oneshot};
use futures::{Async, Future, Poll, Stream};
use inotify::{Event as RawEvent, EventMask, Inotify, WatchDescriptor as RawWatchDescriptor,
              WatchMask};

use {AsyncResult, Error, ErrorKind, Result};

pub struct InotifyService {
    inotify: Inotify,
    command_rx: std_mpsc::Receiver<Command>,
    watches: HashMap<WatchDescriptor, Watch>,
    raw_wd_map: HashMap<RawWatchDescriptor, Vec<WatchDescriptor>>,
    next_wd: WatchDescriptor,
}
impl InotifyService {
    pub fn start() -> Result<InotifyServiceHandle> {
        let (command_tx, command_rx) = std_mpsc::channel();
        let (init_tx, init_rx) = std_mpsc::channel();
        thread::spawn(move || match track!(InotifyService::new(command_rx)) {
            Err(e) => {
                let _ = init_tx.send(Err(e));
            }
            Ok(mut service) => {
                let _ = init_tx.send(Ok(()));
                while track_try_unwrap!(service.run_once()) {}
            }
        });
        track!(init_rx.recv().map_err(Error::from).and_then(|r| r))?;
        Ok(InotifyServiceHandle { command_tx })
    }

    fn new(command_rx: std_mpsc::Receiver<Command>) -> Result<Self> {
        let inotify = track!(Inotify::init().map_err(Error::from))?;
        Ok(InotifyService {
            inotify,
            command_rx,
            watches: HashMap::new(),
            raw_wd_map: HashMap::new(),
            next_wd: WatchDescriptor(0),
        })
    }
    fn run_once(&mut self) -> Result<bool> {
        let mut buf = [0; 4096];
        let mut event_received = false;
        for event in track!(self.inotify.read_events(&mut buf).map_err(Error::from))? {
            event_received = true;
            self.handle_event(event);
        }
        if event_received {
            Ok(true)
        } else {
            match self.command_rx.recv_timeout(Duration::from_millis(10)) {
                Err(RecvTimeoutError::Timeout) => Ok(true),
                Err(RecvTimeoutError::Disconnected) => Ok(false),
                Ok(command) => {
                    track!(self.handle_command(command))?;
                    Ok(true)
                }
            }
        }
    }

    fn handle_event(&mut self, event: RawEvent) {
        if let Some(wds) = self.raw_wd_map.get(&event.wd) {
            for watch in wds.iter().map(|w| &self.watches[w]) {
                watch.notify(&event);
            }
        }
    }
    fn handle_command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::AddWatch {
                path,
                mask,
                event_tx,
                reply,
            } => {
                reply.exit(track!(self.add_watch(path, mask, event_tx)));
            }
            Command::RemoveWatch { wd } => {
                track!(self.remove_watch(wd))?;
            }
        }
        Ok(())
    }
    fn add_watch(
        &mut self,
        path: PathBuf,
        mask: WatchMask,
        event_tx: mpsc::Sender<Event>,
    ) -> Result<WatchDescriptor> {
        track_assert!(
            !mask.contains(
                WatchMask::DONT_FOLLOW | WatchMask::EXCL_UNLINK | WatchMask::MASK_ADD
                    | WatchMask::ONESHOT | WatchMask::ONLYDIR,
            ),
            ErrorKind::InvalidInput,
            "Unsuppported watch mask: {:?}",
            mask
        );

        let raw_wd = track!(
            self.inotify
                .add_watch(&path, mask | WatchMask::MASK_ADD)
                .map_err(Error::from)
        )?;

        let wd = self.next_wd.next();
        let watch = Watch {
            raw_wd: raw_wd.clone(),
            mask,
            event_tx,
        };
        self.watches.insert(wd.clone(), watch);
        self.raw_wd_map
            .entry(raw_wd.clone())
            .or_insert_with(Vec::new)
            .push(wd.clone());
        Ok(wd)
    }
    fn remove_watch(&mut self, wd: WatchDescriptor) -> Result<()> {
        if let Some(watch) = self.watches.remove(&wd) {
            let wds = self.raw_wd_map.get_mut(&watch.raw_wd).expect("Never fails");
            for i in 0..wds.len() {
                if wds[i] == wd {
                    wds.swap_remove(i);
                    break;
                }
            }
            if wds.is_empty() {
                // Updating `EventMask` by calling `Inotify::add_watch` has a risk of
                // causing inconsistency when the inode referred by the path changes,
                // so only deletes when all watches are gone.
                track!(self.inotify.rm_watch(watch.raw_wd).map_err(Error::from))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct InotifyServiceHandle {
    command_tx: std_mpsc::Sender<Command>,
}
impl InotifyServiceHandle {
    pub fn watch<P: AsRef<Path>>(&self, path: P, mask: WatchMask) -> WatchHandle {
        let (reply, monitor) = oneshot::monitor();
        let (event_tx, event_rx) = mpsc::channel();
        let command = Command::AddWatch {
            path: path.as_ref().to_path_buf(),
            mask,
            event_tx,
            reply,
        };
        let _ = self.command_tx.send(command);
        WatchHandle {
            service: self.clone(),
            event_rx,
            state: WatchHandleState::Adding(AsyncResult(monitor)),
        }
    }
}

#[derive(Debug)]
pub struct WatchHandle {
    service: InotifyServiceHandle,
    event_rx: mpsc::Receiver<Event>,
    state: WatchHandleState,
}
impl Drop for WatchHandle {
    fn drop(&mut self) {
        if let WatchHandleState::Watching(wd) = self.state {
            let command = Command::RemoveWatch { wd };
            let _ = self.service.command_tx.send(command);
        }
    }
}
impl Stream for WatchHandle {
    type Item = Event;
    type Error = Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let next_state = if let WatchHandleState::Adding(ref mut future) = self.state {
            if let Async::Ready(wd) = track!(future.poll())? {
                Some(WatchHandleState::Watching(wd))
            } else {
                return Ok(Async::NotReady);
            }
        } else {
            None
        };
        if let Some(next_state) = next_state {
            self.state = next_state;
        }
        track!(self.event_rx.poll().map_err(|_| unreachable!()))
    }
}

#[derive(Debug)]
enum WatchHandleState {
    Adding(AsyncResult<WatchDescriptor>),
    Watching(WatchDescriptor),
}

#[derive(Debug)]
pub struct Event {
    pub mask: EventMask,
    pub cookie: u32,
    pub name: Option<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct WatchDescriptor(u64);
impl WatchDescriptor {
    fn next(&mut self) -> WatchDescriptor {
        let n = self.0;
        self.0 += 1;
        WatchDescriptor(n)
    }
}

#[derive(Debug)]
enum Command {
    AddWatch {
        path: PathBuf,
        mask: WatchMask,
        event_tx: mpsc::Sender<Event>,
        reply: oneshot::Monitored<WatchDescriptor, Error>,
    },
    RemoveWatch {
        wd: WatchDescriptor,
    },
}

#[derive(Debug)]
struct Watch {
    raw_wd: RawWatchDescriptor,
    mask: WatchMask,
    event_tx: mpsc::Sender<Event>,
}
impl Watch {
    fn notify(&self, event: &RawEvent) {
        let mask = EventMask::from_bits_truncate(event.mask.bits() & self.mask.bits());
        if !mask.is_empty() {
            let event = Event {
                mask,
                cookie: event.cookie,
                name: event.name.map(|n| n.to_owned()),
            };
            let _ = self.event_tx.send(event);
        }
    }
}
