//! Errors that can occur while loading a configuration.

use std::backtrace::Backtrace as StdBacktrace;
use std::path::PathBuf;

use camino::Utf8PathBuf;

use crate::config::PatchSetStyle;
use crate::config::url::VcsUrl;

#[derive(thiserror::Error, Debug)]
#[error("Failed to load {kind}{desc_path}", desc_path = self.maybe_desc_path())]
pub struct ConfigLoadError {
    pub(super) kind: super::ConfigKind,
    pub(super) path: Option<PathBuf>,
    #[source]
    pub(super) reason: Box<ConfigLoadErrorReason>,
    #[cfg_attr(use_error_backtrace, backtrace)]
    pub(super) backtrace: StdBacktrace,
}
impl ConfigLoadError {
    fn maybe_desc_path(&self) -> String {
        match self.path {
            None => String::new(),
            Some(ref path) => format!(" from `{}`", path.display()),
        }
    }
    pub fn reason(&self) -> &ConfigLoadErrorReason {
        &self.reason
    }
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum ConfigLoadErrorReason {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Deserialize(#[from] DeserializationError),
    #[error(transparent)]
    Validate(#[from] ConfigValidateError),
}
impl ConfigLoadErrorReason {
    pub fn is_file_not_found(&self) -> bool {
        matches!(self, Self::Io(cause) if matches!(cause.kind(), std::io::ErrorKind::NotFound))
    }
}
impl From<ConfigValidateErrorReason> for ConfigLoadErrorReason {
    fn from(reason: ConfigValidateErrorReason) -> Self {
        Self::Validate(ConfigValidateError(Box::new(reason)))
    }
}

/// Indicates there was an error during deserialization.
#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub struct DeserializationError(pub(crate) toml::de::Error);

/// An error that occurs validating a config object.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ConfigValidateError(Box<ConfigValidateErrorReason>);

#[derive(thiserror::Error, Debug)]
pub(crate) enum ConfigValidateErrorReason {
    #[error("Conflicting local mirrors for `{url}`")]
    #[non_exhaustive]
    ConflictingLocalMirrors {
        url: VcsUrl,
    },
    #[error("Failed to validate configuration for repository {repo_name}")]
    #[non_exhaustive]
    RepoValidate {
        repo_name: String,
        #[source]
        cause: RepoConfigValidateError,
    },
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum RepoConfigValidateError {
    #[error("Multiple patches use the same directory {duplicate:?}")]
    #[non_exhaustive]
    DuplicatePatchDir {
        duplicate: Utf8PathBuf,
    },
    #[error("Failed to validate configuration for {style} patch set {patchdir:?}")]
    #[non_exhaustive]
    PatchSetValidate {
        patchdir: Utf8PathBuf,
        style: PatchSetStyle,
        #[source]
        cause: PatchSetValidateError,
    },
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum PatchSetValidateError {
    #[error("Missing commit message (default cannot be used when there are multiple per-file patchsets)")]
    #[non_exhaustive]
    MustSpecifyCommitMsg,
    #[error("Patch option {field_name} is only appropriate for {expected_style} patches")]
    #[non_exhaustive]
    InappropriateOption {
        field_name: &'static str,
        expected_style: PatchSetStyle,
    },
}
