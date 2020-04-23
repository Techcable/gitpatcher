use git2::{Repository, RepositoryState, Commit};
use std::path::{Path, PathBuf};
use slog::{Logger, info, warn};
use std::str::FromStr;
use git2::build::CheckoutBuilder;
use crate::format_patches::{FormatOptions, PatchFormatter, PatchFormatError};

pub struct PatchFileSet<'a> {
    root_repo: &'a Repository,
    patch_dir: PathBuf,
    patches: Vec<PatchFile>
}
impl<'a> PatchFileSet<'a> {
    pub fn load(target: &'a Repository, patch_dir: &Path) -> Result<Self, PatchError> {
        assert!(patch_dir.is_relative());
        {
            let abs_repo_path = std::fs::canonicalize(target.workdir().unwrap())?;
            let abs_patch_dir = std::fs::canonicalize(patch_dir)?;
            assert!(
                abs_patch_dir.starts_with(&abs_repo_path),
                "Repository path {} must be parent of patch dir {}",
                abs_repo_path.display(),
                abs_patch_dir.display()
            );
        }
        let mut patches = Vec::new();
        for entry in std::fs::read_dir(patch_dir)? {
            let entry = entry?;
            let file_name = match entry.file_name().to_str() {
                Some(file_name) => file_name.to_string(),
                None => continue, // Ignore non-UTF8 paths
            };
            // Ignore all files that aren't patches
            if !file_name.ends_with(".patch") { continue }
            patches.push(PatchFile::parse(patch_dir, &file_name)?);
        }
        Ok(PatchFileSet {
            root_repo: target,
            patches, patch_dir: patch_dir.into()
        })
    }
}
pub struct PatchFile {
    _index: usize,
    path: PathBuf
}
impl PatchFile {
    fn parse(parent: &Path, file_name: &str) -> Result<Self, PatchError> {
        // Must match ASCII regex `[\d]{4}-(commit_name).patch`
        if file_name.len() >= 5 &&
            file_name.as_bytes()[4] == b'-' &&
            file_name.ends_with(".patch") {
            let index = usize::from_str(&file_name[..4])
                .map_err(|_| PatchError::InvalidPatchName { name: file_name.into() })?;
            Ok(PatchFile {
                _index: index, path: parent.join(file_name)
            })
        } else {
            Err(PatchError::InvalidPatchName { name: file_name.into() })
        }
    }
}

#[derive(Default)]
pub struct RegenerateOptions {
    pub format_opts: FormatOptions
}

pub fn regenerate_patches(
    base: &Commit,
    original_patches: &PatchFileSet,
    target: &Repository,
    logger: Logger,
    options: RegenerateOptions
) -> Result<(), PatchError> {
    let target_name = target.path().file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| panic!("Invalid path for target repo: {}", target.path().display()));
    info!(logger, "Formatting patches for {}", original_patches.patch_dir.display());
    // Remove old patches
    match target.state() {
        RepositoryState::Rebase | RepositoryState::RebaseInteractive => {
            warn!(logger, "Rebase detected - partial save");
            let mut rebase = original_patches.root_repo.open_rebase(None)?;
            let next = rebase.operation_current().unwrap_or(0);
            for patch in &original_patches.patches[..next] {
                std::fs::remove_file(&patch.path)?;
            }
        },
        RepositoryState::Clean => {
            for patch in &original_patches.patches {
                std::fs::remove_file(&patch.path)?;
            }
        },
        state => {
            return Err(PatchError::PatchedRepoInvalidState { state });
        }
    }

    // Regenerate the patches
    {
        PatchFormatter::new(
            logger.clone(),
            original_patches.patch_dir.clone(),
            target,
            base.clone(),
            options.format_opts
        )?.generate_all()?
    }

    // TODO: Remove any 'trivial' patches
    if false {
        let mut checkout_patches = CheckoutBuilder::new();
        checkout_patches.recreate_missing(true);
        original_patches.root_repo.checkout_head(Some(&mut checkout_patches))?;
    }

    info!(logger, "Patches for {}", target_name);
    Ok(())
}

#[derive(Debug)]
pub enum PatchError {
    /// The patched repo was in an invalid [RepositoryState]
    PatchedRepoInvalidState {
        state: RepositoryState
    },
    InvalidPatchName {
        name: String
    },
    PatchFormatFailed(PatchFormatError),
    /// An unexpected error occurred using git
    Git(git2::Error),
    Io(std::io::Error)
}
impl From<PatchFormatError> for PatchError {
    fn from(cause: PatchFormatError) -> Self {
        PatchError::PatchFormatFailed(cause)
    }
}
impl From<git2::Error> for PatchError {
    fn from(e: git2::Error) -> Self {
        PatchError::Git(e)
    }
}
impl From<std::io::Error> for PatchError {
    fn from(e: std::io::Error) -> Self {
        PatchError::Io(e)
    }
}
impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::PatchedRepoInvalidState { state } => {
                write!(f, "Target repo is in unexpected state: {:?}", state)
            },
            PatchError::PatchFormatFailed(cause) => {
                write!(f, "Failed to format patches: {}", cause)
            },
            PatchError::InvalidPatchName { name } => {
                write!(f, "Invalid name for patch: {:?}", name)
            },
            PatchError::Git(cause) => {
                write!(f, "Unexpected git error: {}", cause)
            },
            PatchError::Io(cause) => {
                write!(f, "Unexpected IO error: {}", cause)
            },
        }
    }
}