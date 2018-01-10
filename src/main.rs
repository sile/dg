extern crate clap;
extern crate dg;
extern crate fibers;
extern crate futures;
#[macro_use]
extern crate trackable;

use clap::{App, Arg, ArgMatches, SubCommand};
use dg::watch;
use fibers::{Executor, Spawn, ThreadPoolExecutor};
use futures::{Future, Stream};

fn main() {
    let matches = App::new("dg").subcommand(subcommand_watch()).get_matches();
    if let Some(matches) = matches.subcommand_matches("watch") {
        handle_watch(matches);
    } else {
        println!("Usage: {}", matches.usage());
        std::process::exit(1);
    }
}

fn subcommand_watch() -> App<'static, 'static> {
    SubCommand::with_name("watch").arg(Arg::with_name("DIR").index(1).required(true))
}

fn handle_watch(matches: &ArgMatches) {
    let dir = matches.value_of("DIR").unwrap();
    let executor = ThreadPoolExecutor::new().unwrap();
    let mut watcher = watch::fs::FileSystemWatcher::new(executor.handle());
    track_try_unwrap!(watcher.watch(dir));
    let handle = executor.handle();
    executor.spawn(
        watcher
            .for_each(move |mut file| {
                let path = file.path();
                let path2 = file.path();
                println!("WATCH: {:?}", file.path());
                let subscriber = file.subscribe();
                handle.spawn(file.then(|_| Ok(())));
                handle.spawn(
                    subscriber
                        .for_each(move |chunk| {
                            println!("{:?}: {:?}", path, chunk);
                            Ok(())
                        })
                        .then(move |r| {
                            println!("UNWATCH: {:?} ({:?})", path2, r.map_err(|e| e.to_string()));
                            Ok(())
                        }),
                );
                Ok(())
            })
            .then(|_| Ok(())),
    );
    executor.run().unwrap();
}
