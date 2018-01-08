extern crate clap;
extern crate dg;
extern crate futures;
#[macro_use]
extern crate trackable;

use clap::{App, Arg, ArgMatches, SubCommand};
use dg::watch;
use futures::{Async, Stream};

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
    let mut watcher = track_try_unwrap!(watch::fs::FileSystemWatcher::new());
    watcher.add_path(dir);
    loop {
        if let Async::Ready(Some(event)) = track_try_unwrap!(watcher.poll()) {
            println!("TODO: {:?}", event);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
