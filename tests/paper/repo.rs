use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use bstr::{BString, ByteSlice, B};
use git2::{Repository, RepositoryInitOptions};

use crate::paper::repo::SubmoduleId::Bukkit;

pub const PAPER_URL: &str = "https://github.com/PaperMC/Paper.git";
/// Latest commit as of Aug 30th, 2023
pub const PINNED_PAPER_COMMIT: &str = "b4e3b3d1dd447bac4cbf478595c1ec320bc6dd4b";

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
    let repo_dir = repo_cache_dir.join("Paper.git");
    let shared_repo = Repository::init_opts(
        repo_dir,
        RepositoryInitOptions::new()
            // NOTE: NOT bare, because that makes submodules hard
            //.bare(true)
            .mkdir(true)
            .origin_url(PAPER_URL),
    )?;
    assert_eq!(repo_dir, shared_repo.path());
    // generic fetch for everything possible
    duct::cmd!("git", "fetch").dir(&repo_dir).run()?;
    // checkout pinned commit
    duct::cmd!("git", "checkout", "--force", PINNED_PAPER_COMMIT)
        .dir()
        .run()?;
    assert_eq!(
        shared_repo
            .statuses(None)
            .context("Failed to determine repo status")?
            .len(),
        0,
        "Untracked files for repo: {repo_dir:?}"
    );
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
