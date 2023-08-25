use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use git2::build::CheckoutBuilder;
use git2::{ObjectType, Repository, ResetType};
use gitpatcher::apply_patches::EmailMessage;
use gitpatcher::regenerate_patches::PatchFileSet;
use slog::{o, Drain, Level, Logger, OwnedKVList, Record};
use std::convert::Infallible;
use std::env;
use std::ffi::OsStr;
use std::path::PathBuf;

pub struct TerminalDrain;
impl Drain for TerminalDrain {
    type Ok = ();
    type Err = Infallible;

    fn log(&self, record: &Record, _values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        match record.level() {
            Level::Critical | Level::Error | Level::Warning => {
                eprintln!("{}", record.msg());
            }
            Level::Info => {
                println!("{}", record.msg());
            }
            Level::Debug => {
                if std::env::var_os("GITPATCHER_DEBUG").map_or(false, |s| s == "1") {
                    println!("DEBUG: {}", record.msg());
                }
            }
            Level::Trace => {} // Ignore these
        }
        Ok(())
    }
}

#[derive(Parser, Debug)]
#[clap(name = "gitpatcher", about = "A patching system based on git", version = env!("VERGEN_GIT_DESCRIBE"))]
struct GitPatcher {
    #[clap(subcommand)]
    subcommand: PatchSubcommand,
}
#[derive(Subcommand, Debug)]
enum PatchSubcommand {
    /// Apply a single patch file to the current repository
    ApplyPatch(ApplyPatchOpts),
    /// Apply an entire set of patch files to the specified repository
    ApplyAllPatches(ApplyAllPatches),
    /// Regenerate a set of patched files by comparing a patched repo to an upstream reference
    RegeneratePatches(RegeneratePatchOpts),
}

#[derive(Parser, Debug)]
struct ApplyPatchOpts {
    /// The patch file to apply
    patch_file: PathBuf,
    /// The target repository to apply patches too
    ///
    /// Defaults to current directory if nothing is specified
    #[clap(long = "target")]
    target_repo: Option<PathBuf>,
}

#[derive(Parser, Debug)]
struct ApplyAllPatches {
    /// The upstream reference to reset to before applying patches
    #[clap(long)]
    upstream: Option<String>,
    /// The target repository to apply patches too
    target_repo: PathBuf,
    /// The directory containing all the patch files
    patch_dir: PathBuf,
}

#[derive(Parser, Debug)]
struct RegeneratePatchOpts {
    /// The repository containing the patched changes
    patched_repo: PathBuf,
    /// A upstream git reference to compare the patched repo against
    upstream: String,
    /// The directory to place the generated patches in
    patch_dir: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let opt: GitPatcher = GitPatcher::parse();
    match opt.subcommand {
        PatchSubcommand::ApplyPatch(opts) => apply_patch(opts),
        PatchSubcommand::RegeneratePatches(opts) => regenerate_patches(opts),
        PatchSubcommand::ApplyAllPatches(opts) => apply_all_patches(opts),
    }
}

fn apply_all_patches(opts: ApplyAllPatches) -> anyhow::Result<()> {
    let target = Repository::open(&opts.target_repo).with_context(|| {
        format!(
            "Unable to access target repo: {}",
            opts.target_repo.display()
        )
    })?;
    if let Some(ref upstream) = opts.upstream {
        let obj = target
            .resolve_reference_from_short_name(upstream)
            .and_then(|reference| reference.peel(ObjectType::Any))
            .with_context(|| format!("Unable to resolve {upstream:?}"))?;
        let mut checkout = CheckoutBuilder::new();
        checkout.remove_untracked(true);
        target
            .reset(&obj, ResetType::Hard, Some(&mut checkout))
            .with_context(|| format!("Unable to reset to {upstream:?}"))?;
        println!("Reset {} to {}", opts.target_repo.display(), upstream);
    }
    let entries = std::fs::read_dir(&opts.patch_dir)
        .with_context(|| format!("Error accessing patch dir {}", opts.patch_dir.display()))?;
    let mut patch_files = Vec::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("Error accessing patch dir {}", opts.patch_dir.display()))?;
        // Skip all patch files that do not end with '.patch'
        let full_path = entry.path();
        if full_path.extension() != Some(OsStr::new("patch")) {
            continue;
        }
        let raw_name = entry.file_name();
        let s = raw_name.to_str().ok_or_else(|| {
            anyhow!(
                "Invalid patch file name must be UTF8: {:?}",
                entry.file_name()
            )
        })?;
        assert!(s.ends_with(".patch"));
        let patch_name = &s[..(s.len() - ".patch".len())];
        let message_str = std::fs::read_to_string(&full_path)
            .with_context(|| format!("Unable to read patch file {s}"))?;
        let email =
            EmailMessage::parse(&message_str).with_context(|| format!("Invalid patch file {s}"))?;
        patch_files.push((String::from(patch_name), email));
    }
    patch_files.sort_by(|(first, _), (second, _)| first.cmp(second));
    for (name, email) in &patch_files {
        println!("Applying {}.patch", name);
        email
            .apply_commit(&target)
            .with_context(|| format!("Failed to apply patch: {name:?}"))?;
    }
    println!("Successfully applied {} patches!", patch_files.len());
    Ok(())
}

fn apply_patch(opts: ApplyPatchOpts) -> anyhow::Result<()> {
    let target_repo = match opts.target_repo {
        Some(location) => location,
        None => env::current_dir().context("Unable to detect current dir")?,
    };
    let target_repo = Repository::open(&target_repo)
        .with_context(|| format!("Unable to access target repo {}", target_repo.display(),))?;
    let message = std::fs::read_to_string(&opts.patch_file).context("Unable to read patch")?;
    let message = EmailMessage::parse(&message).context("Error parsing patch")?;
    message
        .apply_commit(&target_repo)
        .context("Unable to apply patch")?;
    println!("Applied: {}", opts.patch_file.display());
    Ok(())
}

fn regenerate_patches(opts: RegeneratePatchOpts) -> anyhow::Result<()> {
    let patched_repo = Repository::open(&opts.patched_repo).with_context(|| {
        format!(
            "Unable to access patched repo: {}",
            opts.patched_repo.display()
        )
    })?;
    let upstream_obj = patched_repo
        .resolve_reference_from_short_name(&opts.upstream)
        .and_then(|reference| reference.peel(ObjectType::Any))
        .with_context(|| format!("Unable to resolve upstream ref {:?}", opts.upstream))?;
    let base_repo =
        Repository::discover(&opts.patch_dir).context("Unable to discover repo for patch dir")?;
    let mut patches =
        PatchFileSet::load(&base_repo, &opts.patch_dir).context("Unable to load patches")?;
    let upstream_commit = upstream_obj.as_commit().with_context(|| {
        format!("Upstream ref must be either a tree or a commit: {upstream_obj:?}")
    })?;
    ::gitpatcher::regenerate_patches::regenerate_patches(
        upstream_commit,
        &mut patches,
        &patched_repo,
        Logger::root(TerminalDrain.ignore_res(), o!()),
        Default::default(),
    )?;
    println!("Success!");
    Ok(())
}
