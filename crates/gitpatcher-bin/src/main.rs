use std::env;
use std::path::PathBuf;

use anyhow::{Context, anyhow};
use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use gitpatcher::engine::{ApplyPatchesOptions, PatchEngine, TargetRepo};
use gitpatcher::vcs::VcsRepo;
use slog::{Drain, Logger};

/// Default paths to primary config.
///
/// Keep in sync with [`GitPatcher::primary_config_path`] docs.
const PRIMARY_CONFIG_DEFAULT_PATHS: &[&str] = &["gitpatcher.toml", ".config/gitpatcher.toml"];

#[derive(Parser, Debug)]
#[clap(name = "gitpatcher", about = "A patching system based on git", version = env!("VERGEN_GIT_DESCRIBE"))]
struct GitPatcher {
    /// The root repository, used to infer configuration and commit regenerated patches.
    ///
    /// A subdirectory of the repository root can be specified,
    /// and the root will be automatically detected from there.
    /// By default, this is determined using the current working directory.
    ///
    /// This option mirrors the behavior of git's `-C` option.
    #[clap(long = "root-repo", alias = "root", short = 'C')]
    root_repo_path: Option<PathBuf>,
    /// The primary repo configuration file.
    ///
    /// If not specified, the following default locations will be searched
    /// (resolved relative to the `--root-repo` path)
    /// - `gitpatcher.toml`
    /// - `.config/gitpatcher.toml`
    #[clap(long = "primary-config", alias = "config")]
    primary_config_path: Option<PathBuf>,
    #[clap(subcommand)]
    subcommand: PatchSubcommand,
}
#[derive(Subcommand, Debug)]
enum PatchSubcommand {
    /// Apply configured patches.
    Apply(ApplyOpts),
}

#[derive(Args, Debug)]
struct CommonOpts {
    /// The target repository names.
    ///
    /// If no targets are specified,
    /// all repos will be used.
    #[clap(long = "target")]
    target_repos: Option<Vec<String>>,
}
impl CommonOpts {
    fn resolve_targets<'a>(&self, ctx: &'a CmdContext) -> anyhow::Result<Vec<TargetRepo<'a>>> {
        if let Some(ref target_repos) = self.target_repos {
            assert!(!target_repos.is_empty(), "names should be either `None` or nonempty");
            target_repos
                .iter()
                .map(|name| ctx.engine.target_repo(name))
                .collect::<Result<Vec<_>, _>>()
                .map_err(anyhow::Error::from)
        } else {
            Ok(ctx
                .engine
                .primary_config()
                .repos
                .keys()
                .map(|name| ctx.engine.target_repo(name).expect("name listed in config"))
                .collect())
        }
    }
}

#[derive(Parser, Debug)]
struct ApplyOpts {
    #[clap(flatten)]
    common: CommonOpts,
    /// Initialize repositories if not already setup.
    #[clap(long)]
    init: bool,
    /// Forcibly override unstaged changes in the working directory.
    #[clap(long)]
    force: bool,
}

struct CmdContext<'a> {
    _root_logger: Logger,
    _cli: &'a GitPatcher,
    _root_repo_path: Utf8PathBuf,
    engine: PatchEngine,
}
impl<'a> CmdContext<'a> {
    fn setup(root_logger: &Logger, cli: &'a GitPatcher) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir().context("Failed to determine CWD")?;
        let root_repo_path = cli.root_repo_path.clone().unwrap_or(cwd);
        let root_repo =
            VcsRepo::detect(&root_repo_path).with_context(|| format!("Failed to detect repo at {root_repo_path:?}"))?;
        let root_repo_path = root_repo.workdir().to_owned();
        let primary_config_path = match cli.primary_config_path {
            Some(ref explicit) => explicit.clone(),
            None => PRIMARY_CONFIG_DEFAULT_PATHS
                .iter()
                .map(|&relative_path| root_repo_path.join(relative_path))
                .find(|x| x.exists())
                .ok_or_else(|| anyhow!("Unable to detect gitpatcher config at {root_repo_path:?}"))?
                .into_std_path_buf(),
        };
        let engine = PatchEngine::builder(root_logger, root_repo)
            .primary_config_path(&primary_config_path)
            .init()?;
        Ok(CmdContext {
            _root_repo_path: root_repo_path,
            _root_logger: root_logger.clone(),
            _cli: cli,
            engine,
        })
    }
}

fn main() -> anyhow::Result<()> {
    let cli: GitPatcher = GitPatcher::parse();
    let mut decorator = slog_term::TermDecorator::new();
    if env::var_os("FORCE_COLOR").is_some_and(|x| !x.is_empty()) {
        decorator = decorator.force_color();
    }
    let root_logger = Logger::root(
        std::sync::Mutex::new(slog_term::CompactFormat::new(decorator.build()).build()).fuse(),
        slog::o!(),
    );
    let _guard = slog_scope::set_global_logger(root_logger.clone());
    let ctx = CmdContext::setup(&root_logger, &cli)?;
    match &cli.subcommand {
        PatchSubcommand::Apply(opts) => apply_patch(&ctx, opts),
    }
}

fn apply_patch(ctx: &CmdContext, opts: &ApplyOpts) -> anyhow::Result<()> {
    let targets = opts.common.resolve_targets(ctx)?;
    for target in targets {
        let opts = ApplyPatchesOptions::builder().init(opts.init).force(opts.force).build();
        target
            .apply_patches(&opts)
            .with_context(|| format!("Failed to apply patches to {}", target.name()))?;
    }
    Ok(())
}
