use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::format;
use std::ops::Sub;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use bstr::{BString, ByteSlice, B};
use git2::{Repository, RepositoryInitOptions};
use testdir::private::cargo_metadata::DependencyKind::Build;

use crate::paper::repo::SubmoduleId::Bukkit;

pub const PAPER_URL: &str = "https://github.com/PaperMC/Paper.git";
/// Latest commit as of Aug 30th, 2023
pub const PINNED_PAPER_COMMIT: &str = "b4e3b3d1dd447bac4cbf478595c1ec320bc6dd4b";

pub fn path_to_utf8(path: &Path) -> anyhow::Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("Path is not UTF8: {:?}", path.display()))
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, derive_more::FromStr)]
pub enum SubmoduleId {
    Bukkit,
    Spigot,
    CraftBukkit,
    BuildData,
}
impl SubmoduleId {
    pub fn name(&self) -> &'static str {
        self.full_name().strip_prefix("work/").unwrap()
    }
    pub fn full_name(&self) -> &'static str {
        macro_rules! full_names {
            ($tgt:expr; $($name:ident),*) => {
                match $tgt {
                    $(SubmoduleId::$name => concat!("work/", stringify!($name)),)*
                }
            };
        }
        full_names!(self; Bukkit, Spigot, CraftBukkit)
    }
    pub const ALL: [SubmoduleId; 4] = {
        use self::SubmoduleId::*;
        [Bukkit, Spigot, CraftBukkit, BuildData]
    };
    pub fn from_short_name(short: &str) -> Result<SubmoduleId, InvalidSubmoduleName> {
        Self::ALL
            .iter()
            .copied()
            .find(|submod| submod.name() == short)
            .ok_or_else(|| InvalidSubmoduleName::UnknownName { text: short.into() })
    }
    pub fn from_full_name(full: &str) -> Result<SubmoduleId, InvalidSubmoduleName> {
        Self::from_short_name(
            full.strip_prefix("work/")
                .ok_or_else(InvalidSubmoduleName::MissingPrefix { text: full.into() })?,
        )
    }
}
#[derive(thiserror::Error, Debug)]
enum InvalidSubmoduleName {
    #[error("Invalid submodule name missing `work/` prefix: {text:?}")]
    MissingPrefix { text: String },
    #[error("Invalid submodule name: {text:?}")]
    UnknownName { text: String },
}

struct SharedRepo {
    repo_cache_dir: PathBuf,
    paper_repo: Repository,
    submodules: HashMap<SubmoduleId, Repository>,
}
pub fn setup_shared_repo() -> anyhow::Result<SharedRepo> {
    // This is where the cache magic happens
    let repo_cache_dir = ::scratch::path("papermc-repo-cache");
    let shared_repo = Repository::init_opts(
        repo_cache_dir.join("Paper.git"),
        RepositoryInitOptions::new()
            .bare(true)
            .mkdir(true)
            .origin_url(PAPER_URL),
    )?;
    if shared_repo.find_reference(PINNED_PAPER_COMMIT).is_err() {
        shared_repo
            .find_remote("origin")
            .context("Can't resolve remote `origin`")?
            .fetch(
                &["master", PINNED_PAPER_COMMIT],
                None,
                Some("sync shared repo"),
            )
            .with_context(|| {
                format!("Failed to fetch commit {PINNED_PAPER_COMMIT:?} from remote")
            })?;
    }
    let pinned_commit = shared_repo
        .find_reference(PINNED_PAPER_COMMIT)
        .and_then(|reference| reference.peel_to_commit())
        .with_context(|| format!("Unable to resolve commit {PINNED_PAPER_COMMIT:?}"))?;
    let mut shared_master_branch = shared_repo
        .branch("master", &pinned_commit, true)
        .with_context(|| format!("Failed to set master branch to {pinned_commit:?}"))?;
    shared_master_branch
        .set_upstream(Some("origin/master"))
        .context("Failed to set upstream branch for shared repo")?;
    {
        let submodules = shared_repo
            .submodules()
            .context("Unable to resolve submodules")?;
    }
    let mut shared_submodules = shared_repo
        .submodules()
        .context("Unable to resolve submodules")?;
    let mut res_submodule = HashMap::with_capacity(SubmoduleId::ALL.len());
    for submodule in &mut shared_submodules {
        let name = submodule
            .name_bytes()
            .to_str()
            .context("Failed to convert submodule name")?;
        let resolved_id =
            SubmoduleId::from_full_name(name).context("Unexpected submodule name for PaperMC")?;
        let repo = submodule.repo_init(true)?;
        submodule
            .clone(None)
            .with_context(|| format!("Failed to clone submodule {name:?}"))?;
        match res_submodule.entry(resolved_id) {
            Entry::Occupied(_) => {
                panic!("Conflicting entries for {resolved_id:?}");
            }
            Entry::Vacant(entry) => {
                entry.insert(repo);
            }
        }
    }
    Ok(SharedRepo {
        repo_cache_dir,
        paper_repo: shared_repo,
        submodules,
    })
}

pub struct PaperRepo {
    paper_repo: Repository,
    submodules: HashMap<SubmoduleId, Repository>,
}
pub fn setup_repo(target: &Path) -> anyhow::Result<Repository> {
    let shared_repo = setup_shared_repo().context("Failed to setup the \"shared\" Paper repo")?;
    let result_repo = Repository::init_opts(
        target,
        RepositoryInitOptions::new()
            .bare(false)
            .mkpath(false) // don't make parents
            .mkdir(true) // but do make the directory itself
            .no_reinit(true), // fail if already exists
    )
    .context("Failed to initialize result repo")?;
    {
        let mut result_remote =
            result_repo.remote("upstream", path_to_utf8(&shared_repo.shared_repo_dir)?)?;
        result_remote
            .fetch(&["master"], None, None)
            .with_context(|| {
                format!(
                    "Failed to fetch `master` branch for {:?} @ {:?}",
                    result_remote.name(),
                    result_remote
                        .fetch_refspecs()
                        .map(|arr| Vec::from_iter(arr.iter_bytes().map(BString::from)))
                )
            })?;
    }
    {
        let mut result_submodules = HashMap::new();
        let pending_submodules = result_repo
            .submodules()
            .context("Failed to resolve submodules for result repo")?;
        assert_eq!(pending_submodules.len(), shared_repo.submodules.len());
        for mut submodule in pending_submodules {
            let name = std::str::from_utf8(submodule.name_bytes())
                .context("Submodule name is not valid UTF8")?;
            let id = SubmoduleId::from_full_name(name).context("Unknown submodule")?;
            let original_submodule_repo = &shared_repo.submodules[&id];
            let submod_init = submodule
                .repo_init(true)
                .with_context(|| format!("Initializing submodule {id:?}"))?;
            match result_submodules.entry(id) {
                Entry::Occupied(_) => panic!("Duplicate ids {id:?}"),
                Entry::Vacant(entry) => entry.insert(submod_init),
            }
        }
    }
    Ok(result_repo)
}
