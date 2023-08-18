extern crate clap;
extern crate dg;
extern crate fibers;
extern crate fibers_tasque;
extern crate futures;
#[macro_use]
extern crate trackable;

use std::path::PathBuf;

use clap::Parser;
use dg::{agent, watch};
use fibers::{Executor, InPlaceExecutor, Spawn};
use futures::{Future, Stream};

#[derive(Parser)]
enum Args {
    Watch { dir: PathBuf },
    Agent { dir: PathBuf },
}

fn main() {
    let args = Args::parse();

    match args {
        Args::Watch { dir } => {
            handle_watch(dir);
        }
        Args::Agent { dir } => {
            handle_agent(dir);
        }
    }
}

fn handle_watch(dir: PathBuf) {
    let executor = InPlaceExecutor::new().unwrap();
    let mut watcher = watch::fs::FileSystemWatcher::new(executor.handle());
    track_try_unwrap!(watcher.watch(dir));
    let handle = executor.handle();
    executor.spawn(
        watcher
            .for_each(move |file| {
                handle.spawn(file.for_each(|_content| Ok(())).map_err(|_e| ()));
                Ok(())
            })
            .then(|_| Ok(())),
    );
    executor.run().unwrap();
}

fn handle_agent(dir: PathBuf) {
    let executor = InPlaceExecutor::new().unwrap();
    let mut watcher = watch::fs::FileSystemWatcher::new(executor.handle());
    track_try_unwrap!(watcher.watch(dir));

    fibers_tasque::DefaultIoTaskQueue.get().set_worker_count(1);
    let agent = agent::Agent::new(executor.handle(), watcher);
    executor.spawn(agent.map_err(|e| panic!("{}", e)));
    executor.run().unwrap();
}
