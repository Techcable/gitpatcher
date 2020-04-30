use gitpatcher::apply_patches::{EmailMessage};
use slog::{Drain, OwnedKVList, Record, Level, Logger, o};
use std::convert::Infallible;
use git2::Repository;
use std::path::Path;
use std::process::exit;
use std::env;

const HELP: &str = "./regenerate_patches <patch_name>";
const DEBUG: bool = false;

pub struct TerminalDrain;
impl Drain for TerminalDrain {
    type Ok = ();
    type Err = Infallible;

    fn log(&self, record: &Record, _values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        match record.level() {
            Level::Critical | Level::Error | Level::Warning => {
                eprintln!("{}", record.msg());
            },
            Level::Info => {
                println!("{}", record.msg());
            },
            Level::Debug => {
                if DEBUG {
                    println!("DEBUG: {}", record.msg());
                }
            },
            Level::Trace => {}, // Ignore these
        }
        Ok(())
    }
}

fn main() {
    let args = env::args()
        .skip(1) // Ignore executable name
        .collect::<Vec<_>>();
    if args.len() != 1 {
        eprintln!("Unexpected args: {:?}", args);
        eprintln!("Usage: {}", HELP);
        exit(1);
    }
    let target_repo = Repository::open(env::current_dir()
        .expect("Unable to detect current dir"))
        .unwrap_or_else(|cause| {
            eprintln!("Unable to access patched repo {:?}: {}", &args[0], cause);
            ::std::process::exit(1);
        });
    let patch_file = Path::new(&args[0]);
    let message = std::fs::read_to_string(patch_file).unwrap_or_else(|cause| {
        eprintln!("Unable to read patch: {}", cause);
        exit(1);
    });
    let message = EmailMessage::parse(&message)
        .unwrap_or_else(|cause| {
            eprintln!("Error parsing patch: {}", cause);
            exit(1)
        });
    message.apply_commit(&target_repo).unwrap_or_else(|cause| {
        eprintln!("Unable to apply patch: {}", cause);
        exit(1)
    });
    println!("Applied: {}", patch_file.display())
}
