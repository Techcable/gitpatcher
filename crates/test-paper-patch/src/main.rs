#!/usr/bin/env rust-script --debug
//! Tests gitpatcher by applying & rebuilding patches
//! for the [Paper](https://papermc.io/) minecraft project.
//!
//! Paper normally uses the [paperweight](https://github.com/PaperMC/paperweight)
//! plugin for the [gradle](https://gradle.org/) build tool.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{ensure, Context};
use clap::{Args, Parser, Subcommand};
use slog::{Drain, Logger};

/// Command line interface to the test script.
#[derive(Parser, Debug)]
struct Cli {
    #[command(subcommand)]
    command: RootCommand,
}

/// Stores the logger being used.
static LOGGER: OnceLock<Logger> = OnceLock::new();

fn logger() -> &'static Logger {
    LOGGER.get().unwrap()
}

/// Stores the location of the paper repository,
/// for use by the `gradle` function.
static PAPER_DIR: OnceLock<PathBuf> = OnceLock::new();

fn paper_dir() -> &'static Path {
    PAPER_DIR.get().unwrap()
}

/// Run a gradle command in the paper repo.
///
/// Use exactly like [`duct::cmd`].
fn gradle(args: impl IntoIterator<Item = impl Into<OsString>>) -> duct::Expression {
    duct::cmd(paper_dir().join("gradlew"), args).dir(paper_dir())
}

#[derive(Subcommand, Debug)]
enum RootCommand {
    /// Runs a gradle command in the paper repository.
    Gradle {
        /// The arguments to pass to gradle.
        args: Vec<String>,
    },
    /// Runs an action using gitpatcher, instead of under gradle.
    ///
    /// Several tasks have the same name under both gradle & gitpatcher,
    /// like `applyPatches` and `rebuildPatches`.
    ///
    /// So run `gitpatcher applyPatches` instead of `gradle applyPatches`
    #[command(name = "gitpatcher")]
    GitPatcher {
        #[command(subcommand)]
        command: GitPatcherCommand,
    },
}

#[derive(Subcommand, Debug)]
#[allow(
    // All commands prefixed with `rebuild` for now,
    // which clippy doesn't like
    clippy::enum_variant_names
)]
enum GitPatcherCommand {
    /// Rebuild the API patches using gitpatcher.
    RebuildApiPatches {
        #[command(flatten)]
        opts: GitPatcherRebuildArgs,
    },
    /// Rebuild the server patcehs using gitpatcher.
    RebuildServerPatches {
        #[command(flatten)]
        opts: GitPatcherRebuildArgs,
    },
    /// Rebuild both API and server patches using gitpatcher.
    RebuildPatches {
        #[command(flatten)]
        opts: GitPatcherRebuildArgs,
    },
}

pub fn main() -> anyhow::Result<()> {
    LOGGER
        .set(Logger::root(
            slog_term::FullFormat::new(slog_term::PlainSyncDecorator::new(std::io::stderr()))
                .build()
                .fuse(),
            slog::o!(),
        ))
        .unwrap();
    let args = Cli::parse();
    let root_dir: PathBuf = {
        let file_parent_dir = Path::new(file!())
            .parent()
            .with_context(|| format!("Cannot detect parent of file!() {f:?}", f = file!()))?;
        duct::cmd!("git", "-C", file_parent_dir, "rev-parse", "--show-toplevel")
            .read()?
            .into()
    };
    let work_dir = root_dir.join("work");
    ensure!(work_dir.is_dir(), "Missing `work` directory");
    let paper_dir = work_dir.join("Paper");
    ensure!(paper_dir.is_dir(), "Missing `work/Paper` directory");
    PAPER_DIR.set(paper_dir.clone()).unwrap(); // init global state for gradle! macro
                                               // check for valid HEAD in work/Paper
    duct::cmd!("git", "-C", paper_dir, "rev-parse", "HEAD")
        .stdout_null()
        .run()
        .context("Unable to detect work/Paper repo HEAD")?;
    match args.command {
        RootCommand::Gradle { args } => {
            gradle(&args).run().with_context(|| {
                let args_desc =
                    args.iter()
                        .enumerate()
                        .fold(String::new(), |mut desc, (idx, arg)| {
                            if idx > 0 {
                                desc.push(' ');
                            }
                            desc.push_str(arg);
                            desc
                        });
                format!("Failed to execute command: ./gradlew {args_desc}")
            })?;
            Ok(())
        }
        RootCommand::GitPatcher {
            command: GitPatcherCommand::RebuildPatches { opts },
        } => {
            opts.rebuild_api()?;
            opts.rebuild_server()?;
            Ok(())
        }

        RootCommand::GitPatcher {
            command: GitPatcherCommand::RebuildApiPatches { opts },
        } => {
            opts.rebuild_api()?;
            Ok(())
        }
        RootCommand::GitPatcher {
            command: GitPatcherCommand::RebuildServerPatches { opts },
        } => {
            opts.rebuild_server()?;
            Ok(())
        }
    }
}

#[derive(Args, Debug)]
struct GitPatcherRebuildArgs {
    /// Indicates the result should be staged.
    #[arg(long)]
    stage_result: bool,
}
impl GitPatcherRebuildArgs {
    fn rebuild_api(&self) -> anyhow::Result<()> {
        eprintln!("Rebuilding API patches");
        self.do_rebuild(
            &git2::Repository::open(paper_dir().join("Paper-API"))
                .context("Failed to open API repo")?,
            Path::new("patches/api"),
        )
    }

    fn rebuild_server(&self) -> anyhow::Result<()> {
        eprintln!("Rebuilding server patches");
        self.do_rebuild(
            &git2::Repository::open(paper_dir().join("Paper-Server"))
                .context("Failed to open server repo")?,
            Path::new("patches/server"),
        )
    }

    fn do_rebuild(
        &self,
        target_repo: &git2::Repository,
        relative_patch_dir: &Path,
    ) -> anyhow::Result<()> {
        let paper_repo =
            git2::Repository::open(paper_dir()).context("Failed to open work/Paper repo")?;
        use gitpatcher::regenerate_patches::PatchFileSet;
        let workdir = target_repo.workdir().expect("Missing workdir");
        let base_commit: git2::Commit = target_repo
            .revparse_single("base")
            .and_then(|r| r.peel_to_commit())
            .context("Unable to resolve reference: base")?;
        let mut patch_set = PatchFileSet::load(
            &paper_repo,
            relative_patch_dir.try_into().expect("non-UTF8 path"),
        )
        .with_context(|| {
            format!(
                "Unable to load PatchFileSet from {}",
                relative_patch_dir.display()
            )
        })?;
        gitpatcher::regenerate_patches::regenerate_patches(
            &base_commit,
            &mut patch_set,
            target_repo,
            logger().new(slog::o!(
                "task" => "regenerate_patches",
                "target_repo" => workdir.display().to_string(),
                "relative_patch_dir" => relative_patch_dir.display().to_string(),
            )),
            Default::default(),
        )
        .with_context(|| format!("Failed to regenerate patches from {}", workdir.display()))?;
        if self.stage_result {
            patch_set
                .stage_changes()
                .context("Failed to stage changes")?;
        }
        Ok(())
    }
}
