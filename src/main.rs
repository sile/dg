extern crate clap;
extern crate dg;
extern crate fibers;
extern crate futures;
#[macro_use]
extern crate slog;
extern crate sloggers;
#[macro_use]
extern crate trackable;

use clap::{App, Arg, ArgMatches, SubCommand};
use dg::watch;
use fibers::{Executor, Spawn, ThreadPoolExecutor};
use futures::{Future, Stream};
use slog::Logger;
use sloggers::Build;
use sloggers::types::Severity;

fn main() {
    let matches = App::new("dg")
        .arg(
            Arg::with_name("LOG_LEVEL")
                .long("log-level")
                .takes_value(true)
                .possible_values(&["debug", "info", "warning", "error", "critical"])
                .default_value("info"),
        )
        .subcommand(subcommand_watch())
        .get_matches();

    let log_level = match matches.value_of("LOG_LEVEL").unwrap() {
        "debug" => Severity::Debug,
        "info" => Severity::Info,
        "warning" => Severity::Warning,
        "error" => Severity::Error,
        "critical" => Severity::Critical,
        _ => unreachable!(),
    };
    let logger = track_try_unwrap!(
        sloggers::terminal::TerminalLoggerBuilder::new()
            .level(log_level)
            .destination(sloggers::terminal::Destination::Stderr)
            .build()
    );

    if let Some(matches) = matches.subcommand_matches("watch") {
        handle_watch(matches, logger);
    } else {
        println!("Usage: {}", matches.usage());
        std::process::exit(1);
    }
}

fn subcommand_watch() -> App<'static, 'static> {
    SubCommand::with_name("watch").arg(Arg::with_name("DIR").index(1).required(true))
}

fn handle_watch(matches: &ArgMatches, logger: Logger) {
    let dir = matches.value_of("DIR").unwrap();
    let executor = ThreadPoolExecutor::new().unwrap();
    let mut watcher = watch::fs::FileSystemWatcher::new(logger.clone(), executor.handle());
    track_try_unwrap!(watcher.watch(dir));
    let handle = executor.handle();
    executor.spawn(
        watcher
            .for_each(move |file| {
                let logger = logger.new(o!("file" => format!("{:?}", file.path())));
                let error_logger = logger.clone();
                handle.spawn(file.for_each(move |content| {
                    info!(
                        logger,
                        "Updated: position={}, bytes={}, eof={}",
                        content.offset,
                        content.data.len(),
                        content.eof
                    );
                    Ok(())
                }).map_err(move |e| error!(error_logger, "{}", e)));
                Ok(())
            })
            .then(|_| Ok(())),
    );
    executor.run().unwrap();
}
