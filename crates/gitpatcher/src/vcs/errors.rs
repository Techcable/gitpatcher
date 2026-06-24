use std::fmt::{Display, Formatter};
use std::path::PathBuf;

/// An error that occurs performing a VCS operation.
#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub struct VcsError(Box<vcs_core::Error>);
impl VcsError {
    pub(crate) fn new(cause: vcs_core::Error) -> Self {
        VcsError(Box::new(cause))
    }
}

#[derive(Copy, Clone, Debug)]
pub(super) enum OpenAction {
    Detect,
    Open,
}
impl Display for OpenAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenAction::Detect => f.write_str("detect"),
            OpenAction::Open => f.write_str("open"),
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Failed to {action} VCS repo at {dir:?}")]
pub struct RepoOpenError {
    pub(super) action: OpenAction,
    pub(super) dir: PathBuf,
    #[source]
    pub(super) reason: Box<RepoOpenErrorReason>,
}
impl RepoOpenError {
    /// Indicates the error is caused by the directory not being a repository.
    pub fn is_not_a_repository(&self) -> bool {
        matches!(
            &*self.reason,
            RepoOpenErrorReason::CoreRepoOpen(vcs_core::Error::NotARepository(_))
        )
    }

    /// If this error is caused by the directory not being found.
    pub fn is_dir_not_found(&self) -> bool {
        matches!(&*self.reason, RepoOpenErrorReason::DirectoryNotFound { cause: _ })
    }
}

#[derive(thiserror::Error, Debug)]
pub(super) enum RepoOpenErrorReason {
    #[error(transparent)]
    NonUtf8Path(camino::FromPathBufError),
    #[error(transparent)]
    CoreRepoOpen(vcs_core::Error),
    #[error("Directory not found")]
    DirectoryNotFound {
        #[source]
        cause: std::io::Error,
    },
    /// An error resolving a path which is not [`Self::DirectoryNotFound`].
    #[error("Failed to resolve path")]
    ResolvePathOther {
        #[source]
        cause: std::io::Error,
    },
    #[error("Not a valid repository, although it is inside one ({parent_repo_dir:?})")]
    NotRepoRoot {
        parent_repo_dir: PathBuf,
    },
}
impl RepoOpenErrorReason {
    /// An error calling [`std::fs::canonicalize`].
    pub fn resolve_path(cause: std::io::Error) -> Self {
        if matches!(cause.kind(), std::io::ErrorKind::NotFound) {
            RepoOpenErrorReason::DirectoryNotFound { cause }
        } else {
            RepoOpenErrorReason::ResolvePathOther { cause }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to query repo for working copy changes")]
pub(crate) struct WorkingDirChangesQueryError {
    #[source]
    pub(super) cause: VcsError,
}
