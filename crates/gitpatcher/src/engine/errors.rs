use std::path::PathBuf;

use camino::Utf8PathBuf;

use crate::config::PatchSetStyle;
use crate::config::errors::ConfigLoadError;
use crate::vcs::errors::{TokioInitError, WorkingDirChangesQueryError};
use crate::vcs::{RepoOpenError, VcsError, VcsKind, WorkingDirChanges};

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum PatchEngineInitError {
    #[error("Failed to load preferences")]
    PrefsLoad(#[source] ConfigLoadError),
    #[error("Failed to load primary config from {}", config_path.display())]
    PrimaryConfigLoad {
        config_path: PathBuf,
        #[source]
        cause: ConfigLoadError,
    },
}

#[derive(thiserror::Error, Debug)]
#[error("Failed to apply patches to {repo_name}")]
pub struct ApplyPatchError {
    pub(super) repo_name: String,
    #[source]
    pub(super) reason: Box<ApplyPatchErrorReason>,
}

/// Internal type for [`ApplyPatchError`].
///
/// Not meant to be publicly exposed.
#[derive(thiserror::Error, Debug)]
pub(super) enum ApplyPatchErrorReason {
    #[error(transparent)]
    TokioInit(TokioInitError),
    #[error("Failed to open existing repository (has it been initialized?)")]
    OpenExistingRepo(#[source] RepoOpenError),
    #[error("Failed to initialize repository")]
    InitRepo(#[source] RepoInitError),
    #[error(transparent)]
    WorkingDirChangesQuery(#[from] WorkingDirChangesQueryError),
    #[error("Failed to checkout {rev:?}")]
    Checkout {
        rev: String,
        #[source]
        cause: VcsError,
    },
    #[error("Refusing to override dirty worktree ({desc})", desc = changes.short_desc())]
    DirtyWorktree {
        changes: WorkingDirChanges,
    },
    #[error("Application of patchset {patch_dir} failed")]
    ApplySet {
        patch_dir: Utf8PathBuf,
        #[source]
        cause: PatchSetApplicationError,
    },
    #[error("The \"{style}\" patch style is not currently supported by apply-patch")]
    UnsupportedPatchStyle {
        style: PatchSetStyle,
    },
}

#[derive(thiserror::Error, Debug)]
pub(super) enum RepoInitError {
    #[error("Failed to run `git clone` (revision: {rev:?})")]
    Clone {
        rev: String,
        #[source]
        cause: std::io::Error,
    },
    #[error("Failed to open existing repo and refusing to reinitialize")]
    OpenExisting {
        #[source]
        cause: RepoOpenError,
    },
    #[error("Directory exists, but is not a repository")]
    DirExistsNotRepo {
        #[source]
        cause: RepoOpenError,
    },
    #[error("Failed to setup jj colocated repository")]
    JJInit {
        #[source]
        cause: std::io::Error,
    },
    #[error("Failed to open cloned repo")]
    OpenCloned(#[source] RepoOpenError),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum PatchSetApplicationError {
    #[error("Failed to read patch file from {patch_file}")]
    ReadPatch {
        patch_file: Utf8PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("Failed to parse patch file at {patch_file}")]
    ParsePatch {
        patch_file: Utf8PathBuf,
        #[source]
        cause: diffy::ParsePatchError,
    },
    #[error("Failed to commit files after a successful patch {cause}")]
    Commit {
        #[source]
        cause: VcsError,
    },
    #[error("Failed to run `git add` after successful patch")]
    GitAdd {
        #[source]
        cause: VcsError,
    },
    #[error("Failed to read target file {target_file}")]
    TargetFileRead {
        target_file: Utf8PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("Failed to write applied patch back to {target_file}")]
    TargetFileWrite {
        target_file: Utf8PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("Failed to apply patch {patch_file}")]
    Apply {
        patch_file: Utf8PathBuf,
        #[source]
        cause: diffy::ApplyError,
    },
    #[error(transparent)]
    PatchWalkDir(#[from] PatchDirWalkError),
}

#[derive(thiserror::Error, Debug)]
#[error("Failed to regenerate patches for {repo_name}")]
pub struct RegeneratePatchError {
    pub(super) repo_name: String,
    #[source]
    pub(super) reason: Box<RegeneratePatchErrorReason>,
}

#[derive(thiserror::Error, Debug)]
pub(super) enum RegeneratePatchErrorReason {
    #[error(transparent)]
    TokioInit(TokioInitError),
    #[error("Target repository has uncommitted changes ({desc})", desc = changes.short_desc())]
    DirtyWorkdir {
        changes: WorkingDirChanges,
    },
    #[error(transparent)]
    PatchWalkDir(#[from] PatchDirWalkError),
    #[error("Failed to write patch file to {patch_file:?}")]
    PatchWriteFailed {
        patch_file: Utf8PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("Encountered unsupported binary patch for file {target_file:?}")]
    BinaryPatchUnsupported {
        target_file: Utf8PathBuf,
    },
    #[error("Patch target is not a relative path {target_file:?}")]
    PathTargetNotRelative {
        target_file: Utf8PathBuf,
    },
    #[error("Expected there to be exactly one commit in {log_rev:?}, but got {count}")]
    NotExactlyOneCommit {
        log_rev: String,
        count: usize,
    },
    #[error("Failed to execute {vcs_kind} diff on {diff_spec:?}")]
    RunDiff {
        vcs_kind: VcsKind,
        diff_spec: String,
        #[source]
        cause: std::io::Error,
    },
    #[error("Failed to parse diff generated by {vcs_kind}")]
    ParseVcsDiff {
        vcs_kind: VcsKind,
        #[source]
        cause: diffy::patch_set::PatchSetParseError,
    },
    #[error("Diff generated by {vcs_kind} unexpectedly has a rename ({from} -> {to})")]
    RenameUnexpected {
        vcs_kind: VcsKind,
        from: Utf8PathBuf,
        to: Utf8PathBuf,
    },
    #[error(transparent)]
    WorkingDirChangesQuery(#[from] WorkingDirChangesQueryError),
    #[error("Failed to open repository")]
    OpenRepo(#[source] RepoOpenError),
    #[error("Cannot regenerate per-file patches unless they come first or last (at #{ordinal})", ordinal = index + 1)]
    PerFilePatchesNotFirstLast {
        index: usize,
    },
    #[error("Failed to get repository log")]
    Log(#[source] vcs_core::Error),
    #[error(
        "Per-file patches cannot be adjacent to other per-file patches (both #{first} and #{second})",
        first = first_index + 1,
        second = first_index + 2
    )]
    PerFilePatchesAdjacently {
        first_index: usize,
    },
    #[error("The \"{style}\" patch style is not currently supported by regenerate-patches")]
    UnsupportedPatchStyle {
        style: PatchSetStyle,
    },
    #[error("Unsupported VCS backend \"{}\"", backend.as_str())]
    UnsupportedVcsBackend {
        backend: vcs_core::BackendKind,
    },
    #[error("Failed to delete patch file {patch_file:?}")]
    DeletePatchFile {
        patch_file: Utf8PathBuf,
        #[source]
        cause: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PatchDirWalkError {
    #[error("Failed to walk patch directory ({patch_dir})")]
    WalkDir {
        patch_dir: Utf8PathBuf,
        #[source]
        cause: walkdir::Error,
    },
    #[error("Path for {kind} is not UTF8: {path:?}")]
    PathNotUtf8 {
        kind: &'static str,
        path: PathBuf,
    },
    #[error("Failed to determine relative path of {kind} {path:?} against {root_dir:?}")]
    RelativePathResolution {
        kind: &'static str,
        root_dir: PathBuf,
        path: PathBuf,
        #[source]
        cause: relative_path::RelativeToError,
    },
}
