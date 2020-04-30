use slog::{Drain, OwnedKVList, Record, Level, Logger, o};
use std::convert::Infallible;
use gitpatcher::regenerate_patches::{PatchFileSet};
use std::path::{PathBuf};
use gitpatcher::apply_patches::EmailMessage;
use std::process::exit;
use std::env;
use structopt_derive::StructOpt;
use git2::{Repository, ObjectType};
use structopt::StructOpt as IStructOpt;

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

#[derive(StructOpt)]
#[structopt(about = "A patching system based on git")]
enum GitPatcher {
    /// Apply a single patch file to the current repository
    ApplyPatch(ApplyPatchOpts),
    /// Regenerate a set of patched files by comparing a patched repo to an upstream reference
    RegeneratePatches(RegeneratePatchOpts)
}

#[derive(StructOpt)]
struct ApplyPatchOpts {
    /// The patch file to apply
    #[structopt(parse(from_os_str))]
    patch_file: PathBuf,
    /// The target repository to apply patches too
    ///
    /// Defaults to current directory if nothing is specified
    #[structopt(long = "target", parse(from_os_Str))]
    target_repo: Option<PathBuf>
}

#[derive(StructOpt)]
struct RegeneratePatchOpts {
    /// The repository containing the patched changes
    #[structopt(parse(from_os_str))]
    patched_repo: PathBuf,
    /// A upstream git reference to compare the patched repo against
    upstream: String,
    /// The directory to place the generated patches in
    #[structopt(parse(from_os_str))]
    patch_dir: PathBuf
}

fn main() {
    let opt: GitPatcher = GitPatcher::from_args();
    match opt {
        GitPatcher::ApplyPatch(opts) => apply_patch(opts),
        GitPatcher::RegeneratePatches(opts) => regenerate_patches(opts),
    }
}

fn apply_patch(opts: ApplyPatchOpts) {
    let target_repo = match opts.target_repo {
        Some(location) => location,
        None => env::current_dir().unwrap_or_else(|cause| {
            eprintln!("Unable to detect current dir: {}", cause);
            exit(1);
        })
    };
    let target_repo = Repository::open(target_repo)
        .unwrap_or_else(|cause| {
            eprintln!("Unable to access target repo {}: {}", target_repo.display() cause);
            ::std::process::exit(1);
        });
    let message = std::fs::read_to_string(&opts.patch_file).unwrap_or_else(|cause| {
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
    println!("Applied: {}", opts.patch_file.display())
}

fn regenerate_patches(opts: RegeneratePatchOpts) {
    let patched_repo = Repository::open(&opts.patched_repo)
        .unwrap_or_else(|cause| {
            eprintln!("Unable to access patched repo {:?}: {}", opts.patched_repo, cause);
            ::std::process::exit(1);
        });
    let upstream_obj = patched_repo.resolve_reference_from_short_name(&opts.upstream)
        .and_then(|reference| reference.peel(ObjectType::Any))
        .unwrap_or_else(|cause| {
            eprintln!("Unable to resolve upstream ref {:?}: {}", opts.upstream, cause);
            ::std::process::exit(1);
        });
    let base_repo = Repository::discover(&opts.patch_dir)
        .unwrap_or_else(|e| {
            eprintln!("Unable to discover repo for patch dir: {}", e);
            std::process::exit(1);
        });
    let mut patches = PatchFileSet::load(&base_repo, &opts.patch_dir)
        .unwrap_or_else(|e| {
            eprintln!("Unable to load patches: {}", e);
            std::process::exit(1)
        });
    let upstream_commit = upstream_obj.as_commit()
        .unwrap_or_else(|| {
            eprintln!("Upstream ref must be either a tree or a commit: {:?}", upstream_obj);
            ::std::process::exit(1);
        });
    ::gitpatcher::regenerate_patches::regenerate_patches(
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
