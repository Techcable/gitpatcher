//! Apply an entire set of patches in bulk.
//!
//! Used to implement the the `apply-all-patches` command in the CLI.
use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;

use git2::build::CheckoutBuilder;
use git2::{ObjectType, Repository, ResetType};

use super::email::EmailMessage;
use crate::utils;

pub struct BulkPatchApply<'repo> {
    logger: slog::Logger,
    target_repo: &'repo Repository,
    patch_dir: PathBuf,
}
impl<'repo> BulkPatchApply<'repo> {
    pub fn new(logger: &slog::Logger, target_repo: &'repo Repository, patch_dir: PathBuf) -> Self {
        use utils::log::LogPathValue;
        let logger = logger.new(slog::o!(
            "target_repo" => LogPathValue::from(target_repo.workdir().unwrap_or(target_repo.path())),
            "patch_dir" => LogPathValue::from(&*patch_dir)
        ));
        BulkPatchApply {
            logger,
            target_repo,
            patch_dir,
        }
    }
    /// Reset the target repository to the specified upstream reference.
    ///
    /// This should be done _before_ applying the patches.
    /// It is used to implement the `--upstream` option for the command line.
    pub fn reset_upstream(&self, upstream_name: &str) -> Result<(), ResetUpstreamError> {
        let obj = self
            .target_repo
            .resolve_reference_from_short_name(upstream_name)
            .and_then(|reference| reference.peel(ObjectType::Any))
            .map_err(|cause| ResetUpstreamError::InvalidReference {
                upstream_name: upstream_name.into(),
                cause,
            })?;
        let mut checkout = CheckoutBuilder::new();
        checkout.remove_untracked(true);
        self.target_repo
            .reset(&obj, ResetType::Hard, Some(&mut checkout))
            .map_err(|cause| ResetUpstreamError::FailedReset {
                upstream_name: upstream_name.into(),
                cause,
            })?;
        slog::info!(
            self.logger, "Reset upstream";
            "upstream" => upstream_name,
        );
        Ok(())
    }
    /// Apply all the patches in the directory.
    // TODO: Consider splitting into multiple functions?
    pub fn apply_all(self) -> Result<(), BulkApplyError> {
        let entries = std::fs::read_dir(&self.patch_dir).map_err(|cause| {
            BulkApplyError::ErrorAccessPatchDir {
                cause,
                patch_dir: self.patch_dir.clone(),
            }
        })?;
        /*
         * TODO: Avoid buffering all these patches in-memory
         *
         * Buffering the metadata is fine, but we don't want to do the whole thing.
         */
        struct BufferedPatch {
            patch_name: String,
            patch_file: PathBuf,
            email: EmailMessage,
        }
        let mut patch_files = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|cause| BulkApplyError::ErrorAccessPatchDir {
                cause,
                patch_dir: self.patch_dir.clone(),
            })?;
            let full_patch_path = entry.path();
            // Implicitly skip directory entries that do not end with '.patch'
            if full_patch_path.extension() != Some(OsStr::new("patch")) {
                slog::debug!(
                    self.logger,
                    "Skipping non-patch directory entry";
                    "full_path" => full_patch_path.display(),
                );
                continue;
            }
            let file_name = entry
                .file_name()
                .into_string()
                .map_err(|invalid_file_name| BulkApplyError::PatchNameInvalidUtf8 {
                    raw_entry: PathBuf::from(invalid_file_name),
                })?;
            let patch_name = file_name
                .strip_suffix(".patch")
                .unwrap_or_else(|| panic!("Patch file doesn't end with `.patch`: {file_name:?}"));
            let patch_file_contents =
                String::from_utf8(std::fs::read(&full_patch_path).map_err(|cause| {
                    BulkApplyError::FailedReadPatch {
                        cause,
                        patch_file: full_patch_path.clone(),
                    }
                })?)
                .map_err(|cause| BulkApplyError::PatchContentsInvalidUtf8 {
                    cause,
                    patch_file: full_patch_path.clone(),
                })?;
            let email = EmailMessage::parse(&patch_file_contents).map_err(|cause| {
                BulkApplyError::FailedParsePatch {
                    patch_file: full_patch_path.clone(),
                    cause,
                }
            })?;
            patch_files.push(BufferedPatch {
                email,
                patch_file: full_patch_path,
                patch_name: patch_name.into(),
            });
        }
        /*
         * TODO: Special handling for patch numbering?
         *
         * Maybe the implicit handling by the sort function is enough.
         */
        patch_files.sort_by(|first, second| first.patch_name.cmp(&second.patch_name));
        for patch in &patch_files {
            slog::info!(
                self.logger,
                "Applying patch";
                "patch_name" => &patch.patch_name,
                "patch_file" => patch.patch_file.display()
            );
            patch
                .email
                .apply_commit(self.target_repo)
                .map_err(|cause| BulkApplyError::FailedApplyPatch {
                    cause,
                    name: patch.patch_name.clone(),
                })?;
        }
        slog::info!(
            self.logger,
            "Successfully applied {} patches!",
            patch_files.len()
        );
        Ok(())
    }
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum BulkApplyError {
    #[error("Error accessing patch directory: {}", patch_dir.display())]
    ErrorAccessPatchDir {
        patch_dir: PathBuf,
        #[source]
        cause: io::Error,
    },
    #[error("Patch name must be valid UTF8: {}", raw_entry.display())]
    PatchNameInvalidUtf8 { raw_entry: PathBuf },
    #[error("Failed to read patch file: {}", patch_file.display())]
    FailedReadPatch {
        patch_file: PathBuf,
        #[source]
        cause: io::Error,
    },
    #[error("Patch contents are not valid UTF8: {}", patch_file.display())]
    PatchContentsInvalidUtf8 {
        patch_file: PathBuf,
        #[source]
        cause: std::string::FromUtf8Error,
    },
    #[error("Failed parse patch file: {}", patch_file.display())]
    FailedParsePatch {
        patch_file: PathBuf,
        #[source]
        cause: super::email::InvalidEmailMessage,
    },
    #[error("Failed to apply patch: {name:?}")]
    FailedApplyPatch {
        name: String,
        #[source]
        cause: super::email::PatchApplyError,
    },
}

/// An error that occurs in [BulkPatchApply::reset_upstream].
///
/// This is seperated from the main error type,
/// because resetting to the upstream reference is optional
/// and logically a separate operation.
#[derive(thiserror::Error, Debug)]
pub enum ResetUpstreamError {
    #[error("Unable to resolve reference: {upstream_name:?}")]
    InvalidReference {
        upstream_name: String,
        #[source]
        cause: git2::Error,
    },
    #[error("Failed to reset to {upstream_name:?}")]
    FailedReset {
        upstream_name: String,
        #[source]
        cause: git2::Error,
    },
}
