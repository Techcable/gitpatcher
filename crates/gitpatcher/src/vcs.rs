//! The VCS interface.
//!
//! This is currently implemented in terms of [`vcs_core`],
//! so supports both git and [jujutsu](https://jj-vcs.dev) repositories.
//! However, `vcs_core` is a private dependency and should not be directly exposed.

use std::fmt::{Debug, Display, Formatter};
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use slog::{Key, Record, Serializer};
use vcs_core::FileChange;

use crate::vcs::errors::{OpenAction, RepoOpenErrorReason, WorkingDirChangesQueryError};

pub mod errors;

pub use self::errors::{RepoOpenError, VcsError};

/// The kind of version control system that a [`VcsRepo`] uses.
#[non_exhaustive]
pub enum VcsKind {
    /// The repository uses [git](https://git-scm.com/).
    Git,
    /// The repository uses [jujitsu](https://jj-vcs.dev/).
    ///
    /// If the repository is colocated,
    /// it may also be usable as a git repository.
    Jujutsu,
}

/// A version-controlled repository.
///
/// This is similar to [`gix::Repository`] or [`git2::Repository`],
/// but currently implemented in terms of [`vcs_core::Repo`].
///
/// Bare repos are intentionally unsupported,
/// although right now there is a poor error message due to [ZelAnton/vcs-toolkit-rs#6].
///
/// [`gix::Repository`]: https://docs.rs/gix/0.85/gix/struct.Repository.html
/// [`git2::Repository`]: https://docs.rs/git2/0.21/git2/struct.Repository.html
/// [ZelAnton/vcs-toolkit-rs#6]: https://github.com/ZelAnton/vcs-toolkit-rs/issues/6
#[derive(Debug)]
pub struct VcsRepo {
    /// The root working directory of the repository.
    workdir: Utf8PathBuf,
    repo: vcs_core::Repo,
}
impl VcsRepo {
    /// Open a repository based on its working directory.
    pub fn open(working_dir: impl AsRef<Path>) -> Result<Self, RepoOpenError> {
        let working_dir = working_dir.as_ref();
        let action = OpenAction::Open;
        let create_error = |reason: RepoOpenErrorReason| RepoOpenError {
            // uses relative path for consistency with `detect`
            dir: working_dir.to_path_buf(),
            action,
            reason: Box::new(reason),
        };
        // resolve the working directory to make path comparisons work
        let resolved_working_dir = std::fs::canonicalize(working_dir)
            .map_err(RepoOpenErrorReason::resolve_path)
            .map_err(&create_error)?;
        let detected = Self::detect(working_dir).map_err(|mut cause| {
            cause.action = action;
            cause
        })?;
        if detected.workdir() == resolved_working_dir {
            Ok(detected)
        } else {
            Err(create_error(RepoOpenErrorReason::NotRepoRoot {
                parent_repo_dir: detected.workdir().into(),
            }))
        }
    }

    /// Detect a repository based on a subdirectory.
    ///
    /// In a git repository this is functionally equivalent
    /// to [`gix::discover`] or `git rev-parse --show-toplevel`.
    ///
    /// [`gix::discover`]: https://docs.rs/gix/latest/gix/fn.discover.html
    pub fn detect(dir: &Path) -> Result<Self, RepoOpenError> {
        let create_error = |reason: RepoOpenErrorReason| RepoOpenError {
            action: OpenAction::Detect,
            reason: Box::new(reason),
            dir: dir.to_path_buf(),
        };
        let dir = Utf8PathBuf::try_from(dir.to_path_buf())
            .map_err(RepoOpenErrorReason::NonUtf8Path)
            .map_err(&create_error)?;
        // NOTE: Right now bare repos give a bad error message (vcs-toolkit-rs#6)
        let repo = vcs_core::Repo::open(&dir)
            .map_err(RepoOpenErrorReason::CoreRepoOpen)
            .map_err(&create_error)?;
        let workdir = repo.root();
        let workdir = Utf8PathBuf::try_from(workdir.to_path_buf())
            .map_err(RepoOpenErrorReason::NonUtf8Path)
            .map_err(&create_error)?;
        assert!(workdir.is_absolute(), "workdir is still relative {workdir:?}");
        Ok(VcsRepo { workdir, repo })
    }

    /// The root working directory of the repository.
    ///
    /// This is always present, as bare repositories are not supported.
    #[inline]
    pub fn workdir(&self) -> &Utf8Path {
        &self.workdir
    }

    /// Access this repository as a [`vcs_core::Repo`].
    #[inline]
    pub(crate) fn core_repo(&self) -> &vcs_core::Repo {
        &self.repo
    }

    pub(crate) async fn query_workdir_changes(&self) -> Result<WorkingDirChanges, WorkingDirChangesQueryError> {
        self.core_repo()
            .changed_files()
            .await
            .map(|mut changes| {
                // sort for determinism
                changes.sort_by_key(|file| file.path.clone());
                WorkingDirChanges { changes }
            })
            .map_err(|cause| WorkingDirChangesQueryError {
                cause: VcsError::new(cause),
            })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkingDirChanges {
    changes: Vec<FileChange>,
}
impl WorkingDirChanges {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// A short description for use in errors.
    ///
    /// Intended to be wrapped in parens.
    pub fn short_desc(&self) -> impl Display + '_ {
        struct Desc<'a> {
            num_changes: usize,
            first_file: Option<&'a FileChange>,
        }
        impl Display for Desc<'_> {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                if let Some(first_file) = self.first_file {
                    write!(
                        f,
                        "changed {num_changes} files, first one is {first_file:?}",
                        num_changes = self.num_changes,
                        first_file = first_file.path,
                    )
                } else {
                    assert_eq!(self.num_changes, 0);
                    f.write_str("No changes")
                }
            }
        }
        Desc {
            num_changes: self.changes.len(),
            first_file: self.changes.first(),
        }
    }
}
impl slog::Value for WorkingDirChanges {
    fn serialize(&self, record: &Record<'_>, key: Key, serializer: &mut dyn Serializer) -> slog::Result {
        #[derive(serde::Serialize, Clone)]
        struct Desc {
            count: usize,
            #[serde(skip_serializing_if = "Option::is_none")]
            first_file: Option<String>,
        }
        slog::Value::serialize(
            &slog::Serde(Desc {
                count: self.changes.len(),
                first_file: self.changes.first().map(|change| change.path.display().to_string()),
            }),
            record,
            key,
            serializer,
        )
    }
}

#[cfg(test)]
mod test {
    use testdir::testdir;

    use super::*;

    #[test]
    fn open_error_not_exists() {
        let testdir = testdir!();
        let missing_dir = testdir.join("missing");
        assert!(!missing_dir.exists());
        let err = VcsRepo::open(&missing_dir).unwrap_err();
        assert!(err.is_dir_not_found(), "{err:?}");
    }

    #[test]
    fn open_error_not_repo() {
        let testdir = testdir!();
        let empty_dir = testdir.join("empty");
        assert!(!empty_dir.exists());
        std::fs::create_dir(&empty_dir).unwrap();
        let err = VcsRepo::open(&empty_dir).unwrap_err();
        assert!(err.is_not_a_repository(), "{err:?}");
    }
}
