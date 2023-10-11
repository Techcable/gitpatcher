use std::env;
use std::path::PathBuf;

use anyhow::Context;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use git2::{ObjectType, Repository};
use gitpatcher::apply_patches::bulk::BulkPatchApply;
use gitpatcher::apply_patches::EmailMessage;
use gitpatcher::regenerate_patches::PatchFileSet;
use slog::{Drain, Logger};

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
    patch_dir: Utf8PathBuf,
}

fn main() -> anyhow::Result<()> {
    let opt: GitPatcher = GitPatcher::parse();
    let plain = slog_term::PlainSyncDecorator::new(std::io::stdout());
    let logger = Logger::root(
        std::sync::Mutex::new(slog_term::CompactFormat::new(plain).build()).fuse(),
        slog::o!(),
    );
    match opt.subcommand {
        PatchSubcommand::ApplyPatch(opts) => apply_patch(opts),
        PatchSubcommand::RegeneratePatches(opts) => regenerate_patches(logger, opts),
        PatchSubcommand::ApplyAllPatches(opts) => apply_all_patches(logger, opts),
    }
}

fn apply_all_patches(logger: Logger, opts: ApplyAllPatches) -> anyhow::Result<()> {
    let target = Repository::open(&opts.target_repo).with_context(|| {
        format!(
            "Unable to access target repo: {}",
            opts.target_repo.display()
        )
    })?;
    let bulk_apply = BulkPatchApply::new(&logger, &target, opts.patch_dir);
    if let Some(ref upstream) = opts.upstream {
        bulk_apply.reset_upstream(upstream).with_context(|| {
            format!(
                "Failed to reset {} to upstream {upstream:?}",
                opts.target_repo.display()
            )
        })?;
    }
    bulk_apply
        .apply_all()
        .context("Failed to bulk_apply patches")?;
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

fn regenerate_patches(logger: Logger, opts: RegeneratePatchOpts) -> anyhow::Result<()> {
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
        logger.clone(),
        Default::default(),
    )
    .context("Failed to regenerate patches")?;
    println!("Success!");
    Ok(())
}
