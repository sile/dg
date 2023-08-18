extern crate clap;
extern crate dg;
extern crate fibers;
extern crate fibers_tasque;
extern crate futures;
#[macro_use]
extern crate slog;
extern crate sloggers;
#[macro_use]
extern crate trackable;

use std::path::PathBuf;

use clap::Parser;
use dg::{agent, watch};
use fibers::{Executor, InPlaceExecutor, Spawn};
use futures::{Future, Stream};
use slog::Logger;
use sloggers::types::Severity;
use sloggers::Build;

#[derive(Parser)]
enum Args {
    Watch { dir: PathBuf },
    Agent { dir: PathBuf },
}

fn main() {
    let args = Args::parse();
    let log_level = Severity::Debug;
    let logger = track_try_unwrap!(sloggers::terminal::TerminalLoggerBuilder::new()
        .level(log_level)
        .destination(sloggers::terminal::Destination::Stderr)
        .build());

    match args {
        Args::Watch { dir } => {
            handle_watch(dir, logger);
        }
        Args::Agent { dir } => {
            handle_agent(dir, logger);
        }
    }
}

fn handle_watch(dir: PathBuf, logger: Logger) {
    let executor = InPlaceExecutor::new().unwrap();
    let mut watcher = watch::fs::FileSystemWatcher::new(logger.clone(), executor.handle());
    track_try_unwrap!(watcher.watch(dir));
    let handle = executor.handle();
    executor.spawn(
        watcher
            .for_each(move |file| {
                let logger = logger.new(o!("file" => format!("{:?}", file.path())));
                let error_logger = logger.clone();
                handle.spawn(
                    file.for_each(move |content| {
                        info!(
                            logger,
                            "Updated: position={}, bytes={}, eof={}",
                            content.offset,
                            content.data.len(),
                            content.eof
                        );
                        Ok(())
                    })
                    .map_err(move |e| error!(error_logger, "{}", e)),
                );
                Ok(())
            })
            .then(|_| Ok(())),
    );
    executor.run().unwrap();
}

fn handle_agent(dir: PathBuf, logger: Logger) {
    let executor = InPlaceExecutor::new().unwrap();
    let mut watcher = watch::fs::FileSystemWatcher::new(logger.clone(), executor.handle());
    track_try_unwrap!(watcher.watch(dir));

    fibers_tasque::DefaultIoTaskQueue.get().set_worker_count(1);
    let agent = agent::Agent::new(logger, executor.handle(), watcher);
    executor.spawn(agent.map_err(|e| panic!("{}", e)));
    executor.run().unwrap();
}
