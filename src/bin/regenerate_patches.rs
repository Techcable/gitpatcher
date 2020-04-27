use gitpatcher::regenerate_patches::{regenerate_patches, PatchFileSet};
use git2::{Repository, ObjectType};
use std::path::{Path, PathBuf};
use slog::{Drain, OwnedKVList, Record, Level, Logger, o};
use std::convert::Infallible;

const HELP: &str = "./regenerate_patches <patched_rep> <upstream> <patch_dir>";
const DEBUG: bool = true;

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
    let args = std::env::args()
        .skip(1) // Ignore executable name
        .collect::<Vec<_>>();
    if args.len() != 3 {
        eprintln!("Unexpected args: {:?}", args);
        eprintln!("Usage: {}", HELP);
        std::process::exit(1);
    }
    let patched_repo = Repository::open(Path::new(&args[0]))
        .unwrap_or_else(|cause| {
            eprintln!("Unable to access patched repo {:?}: {}", &args[0], cause);
            ::std::process::exit(1);
        });
    let upstream_obj = patched_repo.resolve_reference_from_short_name(&args[1])
        .and_then(|reference| reference.peel(ObjectType::Any))
        .unwrap_or_else(|cause| {
            eprintln!("Unable to resolve upstream ref {:?}: {}", &args[1], cause);
            ::std::process::exit(1);
        });
    let patch_dir = PathBuf::from(&args[2]);
    let base_repo = Repository::discover(&patch_dir)
        .unwrap_or_else(|e| {
            eprintln!("Unable to discover repo for patch dir: {}", e);
            std::process::exit(1);
        });
    let mut patches = PatchFileSet::load(&base_repo, &patch_dir)
        .unwrap_or_else(|e| {
            eprintln!("Unable to load patches: {}", e);
            std::process::exit(1)
        });
    let upstream_commit = upstream_obj.as_commit()
        .unwrap_or_else(|| {
            eprintln!("Upstream ref must be either a tree or a commit: {:?}", upstream_obj);
            ::std::process::exit(1);
        });
    regenerate_patches(
        &upstream_commit,
        &mut patches,
        &patched_repo,
        Logger::root(TerminalDrain.ignore_res(), o!()),
        Default::default()
    ).unwrap_or_else(|e| {
        eprintln!("{}", e);
        std::process::exit(1);
    });
    println!("Success!");
}
