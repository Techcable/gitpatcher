//! Configuration for gitpatcher.
//!
//! There are two separate types of config files [`PrimaryConfig`] and [`PreferencesConfig`]
//! The primary configuration drives the main functionality and should be committed to version control.
//! The "preferences" config allows user-specific or machine-specific tweaks.
//!
//! This mirrors the distinction in cargo between `Cargo.toml` (primary)
//! and `~/.config/cargo/config.toml` (preferences).
#![warn(clippy::exhaustive_enums, clippy::exhaustive_structs)]

use std::backtrace::Backtrace as StdBacktrace;
use std::fmt::{Debug, Display, Formatter};
use std::path::Path;

use camino::Utf8PathBuf;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use relative_path::RelativePathBuf;
use slog::{Key, Record, Serializer, trace};

use crate::config::errors::{
    ConfigLoadError, ConfigLoadErrorReason, ConfigValidateErrorReason, PatchSetValidateError, RepoConfigValidateError,
};
use crate::config::url::VcsUrl;

pub mod dirs;
pub mod errors;
pub mod url;

#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub(crate) enum ConfigKind {
    Preferences,
    Primary,
}
impl ConfigKind {
    fn name(self) -> &'static str {
        match self {
            ConfigKind::Preferences => "preferences",
            ConfigKind::Primary => "primary config",
        }
    }
}
impl Display for ConfigKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
impl slog::Value for ConfigKind {
    fn serialize(&self, _record: &Record<'_>, key: Key, serializer: &mut dyn Serializer) -> slog::Result {
        serializer.emit_str(key, self.name())
    }
}

/// Internal trait for top-level configuration objects.
pub(crate) trait Config: serde::de::DeserializeOwned {
    const KIND: ConfigKind;
    fn load_from(path: &Path) -> Result<Self, ConfigLoadError> {
        let config_error = |reason: ConfigLoadErrorReason| ConfigLoadError {
            reason: Box::new(reason),
            path: Some(path.to_owned()),
            kind: Self::KIND,
            backtrace: StdBacktrace::capture(),
        };
        trace!(
            &slog_scope::logger(),
            "Loading gitpatcher configuration";
            "kind" => Self::KIND,
            "path" => path.display(),
        );
        let text = std::fs::read_to_string(path).map_err(|cause| config_error(cause.into()))?;
        let deserialized = toml::de::from_str::<Self>(&text)
            .map_err(errors::DeserializationError)
            .map_err(|cause| config_error(cause.into()))?;
        deserialized.validate().map_err(|cause| config_error(cause.into()))?;
        Ok(deserialized)
    }
    /// Ensure that this is semantically valid.
    ///
    /// Called after serde deserialization to give more detailed errors.
    fn validate(&self) -> Result<(), ConfigValidateErrorReason>;
}

/// User-customizable preferences that are not part of the [`PrimaryConfig`].
///
/// Stored in `~/.config/gitpatcher/prefs.toml`.
#[derive(serde::Deserialize, Default, Debug)]
#[non_exhaustive]
pub struct PreferencesConfig {
    /// Prefer the use of jj instead of pure git.
    #[serde(default, rename = "prefer-jj")]
    pub prefer_jj: bool,
    #[serde(default)]
    pub mirrors: MirrorPreferences,
}
impl Config for PreferencesConfig {
    const KIND: ConfigKind = ConfigKind::Preferences;
    fn validate(&self) -> Result<(), ConfigValidateErrorReason> {
        use indexmap::map::Entry;
        // validate there are no duplicates
        let mut by_url = IndexMap::<&VcsUrl, &LocalMirror>::new();
        for local in &self.mirrors.local {
            for url in &local.remote_urls {
                match by_url.entry(url) {
                    Entry::Occupied(_) => {
                        return Err(ConfigValidateErrorReason::ConflictingLocalMirrors { url: url.clone() });
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(local);
                    }
                }
            }
        }
        Ok(())
    }
}
impl PreferencesConfig {
    pub fn load() -> Result<Self, ConfigLoadError> {
        let kind = Self::KIND;
        let path = self::dirs::config_dir()
            .map_err(|cause| ConfigLoadError {
                kind,
                reason: Box::new(ConfigLoadErrorReason::Io(cause)),
                path: None, // don't know the path before resolving it
                backtrace: StdBacktrace::capture(),
            })?
            .join("prefs.toml");
        match Self::load_from(&path) {
            Ok(res) => Ok(res),
            Err(e) if e.reason().is_file_not_found() => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }
}

/// Allows defining mirrors for certain repositories.
///
/// Currently, only local mirrors are supported.
#[derive(serde::Deserialize, Default, Debug)]
#[non_exhaustive]
pub struct MirrorPreferences {
    #[serde(default)]
    pub local: Vec<LocalMirror>,
}
impl MirrorPreferences {
    pub(crate) fn find_local_mirror(&self, url: &VcsUrl) -> Option<&LocalMirror> {
        self.local
            .iter()
            .filter(|mirror| mirror.remote_urls.contains(url))
            .at_most_one()
            .expect("duplicate mirrors after validation")
    }
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct LocalMirror {
    /// The URL of the remote to override.
    ///
    /// Whenever any of these URLs match the remote URL,
    /// then the local mirror will be used.
    ///
    /// Duplicate urls are implicitly ignored.
    pub remote_urls: IndexSet<VcsUrl>,
    /// The path of the local repository to use as a mirror.
    pub local_path: Utf8PathBuf,
    /// If this directory is missing, should it be a warning or error?
    ///
    /// By default, it is false, so missing directories are only warnings.
    #[serde(default)]
    pub required: bool,
    /// If the objects in the local mirror should be directly referenced
    /// in the style of `git clone --shared <path>`.
    ///
    /// If this is false (the default), the local mirror will only be used to speed up fetching,
    /// in the style of `git clone --reference <path> --disassociate <real_url>`
    #[serde(default)]
    pub reference: bool,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[non_exhaustive]
pub struct PrimaryConfig {
    /// The map of repositories, indexed by their identifiers.
    pub repos: IndexMap<String, RepoConfig>,
}
impl Config for PrimaryConfig {
    const KIND: ConfigKind = ConfigKind::Primary;
    fn validate(&self) -> Result<(), ConfigValidateErrorReason> {
        for (name, repo) in &self.repos {
            repo.validate()
                .map_err(|cause| ConfigValidateErrorReason::RepoValidate {
                    repo_name: name.into(),
                    cause,
                })?;
        }
        Ok(())
    }
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct RepoConfig {
    /// Override the path to place the patched repository.
    ///
    /// By default, this is set to the repo identifier.
    #[serde(rename = "path", default)]
    pub custom_path: Option<RelativePathBuf>,
    /// The URL of the upstream repository.
    pub remote_url: VcsUrl,
    /// The git ref of to apply patches against.
    pub upstream_ref: String,
    #[serde(rename = "patchsets", alias = "patches")]
    pub patch_sets: Vec<PatchSetConfig>,
}
impl RepoConfig {
    fn validate(&self) -> Result<(), RepoConfigValidateError> {
        let count_per_file_style = self
            .patch_sets
            .iter()
            .filter(|x| x.style == PatchSetStyle::PerFile)
            .count();
        let mut seen_patch_dirs = IndexSet::new();
        for patch_set in &self.patch_sets {
            if !seen_patch_dirs.insert(patch_set.patch_dir.clone()) {
                return Err(RepoConfigValidateError::DuplicatePatchDir {
                    duplicate: Utf8PathBuf::from(patch_set.patch_dir.to_string()),
                });
            }
            patch_set
                .validate(&PatchSetValidateContext {
                    commit_msg_required: count_per_file_style > 1,
                })
                .map_err(|cause| RepoConfigValidateError::PatchSetValidate {
                    style: patch_set.style,
                    cause,
                    patchdir: patch_set.patch_dir.to_string().into(),
                })?;
        }
        Ok(())
    }
}
struct PatchSetValidateContext {
    commit_msg_required: bool,
}

#[derive(Copy, Clone, Debug, serde::Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub enum PatchSetStyle {
    PerFile,
}
impl Display for PatchSetStyle {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PatchSetStyle::PerFile => "per-file",
        })
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub struct PatchSetConfig {
    pub style: PatchSetStyle,
    /// Where the patches are located.
    #[serde(alias = "dir", alias = "patchdir")]
    pub patch_dir: RelativePathBuf,
    /// The generated commit message.
    ///
    /// Only applicable for per-file patches.
    pub commit_msg: Option<String>,
}
impl PatchSetConfig {
    fn validate(&self, ctx: &PatchSetValidateContext) -> Result<(), PatchSetValidateError> {
        macro_rules! restrict_options {
            ($this:expr, $expected_style:expr, { $($field:ident),+ $(,)? }) => ({
                let expected_style: PatchSetStyle = $expected_style;
                if $this.style != expected_style {
                    $(if $this.$field.is_some() {
                        return Err(PatchSetValidateError::InappropriateOption {
                            expected_style,
                            field_name: stringify!($field),
                        })
                    })*
                }
            });
        }
        restrict_options!(self, PatchSetStyle::PerFile, {
            commit_msg,
        });
        if ctx.commit_msg_required && self.commit_msg.is_none() {
            return Err(PatchSetValidateError::MustSpecifyCommitMsg {});
        }
        Ok(())
    }
}
