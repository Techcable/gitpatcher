use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use camino::{Utf8Path, Utf8PathBuf};
use diffy::patch_set::{FileOperation, PatchKind};
use indexmap::IndexMap;
use itertools::Itertools;
use once_cell::sync;
use relative_path::{PathExt, RelativePath, RelativePathBuf};
use slog::{Logger, debug, info, trace, warn};
use walkdir::WalkDir;

use crate::config::{Config, PatchSetConfig, PatchSetStyle, PreferencesConfig, PrimaryConfig, RepoConfig};
use crate::engine::errors::{
    ApplyPatchError, ApplyPatchErrorReason, PatchDirWalkError, PatchEngineInitError, PatchSetApplicationError,
    RegeneratePatchError, RegeneratePatchErrorReason, RepoInitError,
};
use crate::utils::logging::DuctLoggingExt;
use crate::vcs::{VcsError, VcsKind, VcsRepo};

pub mod errors;

#[derive(Default, Clone, Debug, bon::Builder)]
#[non_exhaustive]
pub struct ApplyPatchesOptions {
    /// Whether to initialize the repository if missing.
    pub init: bool,
    /// Whether to ignore working directory changes.
    pub force: bool,
}

#[derive(Default, Clone, Debug, bon::Builder)]
pub struct RegeneratePatchesOptions {
    /// If dirty changes should be implicitly included into the last change rather than causing an error.
    pub include_dirty: bool,
}

#[derive(Debug, Clone)]
pub struct TargetRepo<'a> {
    engine: &'a PatchEngine,
    state: &'a TargetRepoState,
    logger: Logger,
}
impl TargetRepo<'_> {
    #[inline]
    pub fn target_workdir(&self) -> &Utf8Path {
        &self.state.workdir
    }
    #[inline]
    pub fn name(&self) -> &str {
        &self.state.name
    }

    #[inline]
    fn config(&self) -> &RepoConfig {
        &self.state.config
    }

    fn init_repo(&self) -> Result<&'_ VcsRepo, RepoInitError> {
        let logger = &self.logger;
        self.state.repo.get_or_try_init(|| {
            match VcsRepo::open(self.target_workdir()) {
                Ok(repo) => return Ok(repo),
                Err(cause) if cause.is_dir_not_found() => {
                    // fallthrough to init logic
                }
                Err(cause) if cause.is_not_a_repository() => return Err(RepoInitError::DirExistsNotRepo { cause }),
                Err(cause) => return Err(RepoInitError::OpenExisting { cause }),
            }
            let url = &self.config().remote_url;
            let url_str = url.to_string();
            // need to clone so we can use --reference and friends
            let mut args = [
                "clone", "--origin",
                "upstream",
                // don't include --revesion,
                // or else git won't fetch tags
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
            let local_mirror = self.engine.prefs.mirrors.find_local_mirror(url);
            if let Some(mirror) = local_mirror {
                args.push(
                    if mirror.required {
                        "--reference"
                    } else {
                        "--reference-if-able"
                    }
                    .into(),
                );
                let local_path = shellexpand::tilde(mirror.local_path.as_str());
                args.push(local_path.into_owned());
                if !mirror.reference {
                    args.push("--dissociate".into());
                }
            }
            args.extend(["--".into(), url_str.clone(), self.target_workdir().to_string()]);
            info!(
                logger,
                "Cloning and initializing target repository";
                "url" => &url_str,
                "local_mirror" => local_mirror.map(|x| x.local_path.as_str()),
            );
            duct::cmd("git", args)
                .log_on_spawn(logger)
                .dir(self.engine.root_workdir())
                .run()
                .map_err(|cause| RepoInitError::Clone {
                    cause,
                    rev: self.config().upstream_ref.clone(),
                })?;
            if self.engine.prefs.prefer_jj {
                info!(logger, "Setting up jj repository colocation");
                duct::cmd!("jj", "git", "init", "--colocate", ".")
                    .log_on_spawn(logger)
                    .dir(self.target_workdir())
                    .run()
                    .map_err(|cause| RepoInitError::JJInit { cause })?;
            }
            let res = VcsRepo::open(self.target_workdir().as_std_path()).map_err(RepoInitError::OpenCloned)?;
            if self.engine.prefs.prefer_jj {
                assert_eq!(
                    res.core_repo().kind(),
                    vcs_core::BackendKind::Jj,
                    "unexpected backend for {res:?}"
                );
            }
            Ok(res)
        })
    }

    pub fn regenerate_patches(&self, opts: &RegeneratePatchesOptions) -> Result<(), RegeneratePatchError> {
        let logger = &self.logger;
        let create_error = |reason| RegeneratePatchError {
            repo_name: self.state.name.clone(),
            reason: Box::new(reason),
        };
        let target_repo = self
            .state
            .repo
            .get_or_try_init(|| VcsRepo::open(self.target_workdir()))
            .map_err(RegeneratePatchErrorReason::OpenRepo)
            .map_err(&create_error)?;
        let rt = VcsRepo::create_tokio_runtime()
            .map_err(RegeneratePatchErrorReason::TokioInit)
            .map_err(&create_error)?;
        for (index, patch_config) in self.config().patch_sets.iter().enumerate() {
            match patch_config.style {
                PatchSetStyle::PerFile => {
                    if index != 0 && index + 1 != self.config().patch_sets.len() {
                        return Err(create_error(RegeneratePatchErrorReason::PerFilePatchesNotFirstLast {
                            index,
                        }));
                    }
                    if let Some(prev_index) = index.checked_sub(1) {
                        let prev_config = &self.config().patch_sets[prev_index];
                        if matches!(prev_config.style, PatchSetStyle::PerFile) {
                            return Err(create_error(RegeneratePatchErrorReason::PerFilePatchesAdjacently {
                                first_index: prev_index,
                            }));
                        }
                    }
                }
                unsupported @ PatchSetStyle::PerCommit => {
                    return Err(create_error(RegeneratePatchErrorReason::UnsupportedPatchStyle {
                        style: unsupported,
                    }));
                }
            }
        }
        if self.config().patch_sets.is_empty() {
            warn!(logger, "No configured patch sets to regenerate!");
            return Ok(());
        }
        assert_eq!(
            self.config().patch_sets.len(),
            1,
            "Because only per-file patches are supported right now, there should only be one patch set",
        );
        let single_patch_set = &self.config().patch_sets[0];
        rt.block_on(async {
            let changes = target_repo
                .query_workdir_changes()
                .await
                .map_err(RegeneratePatchErrorReason::WorkingDirChangesQuery)?;
            if !changes.is_empty() && !opts.include_dirty {
                return Err(RegeneratePatchErrorReason::DirtyWorkdir { changes });
            }
            let upstream_ref = &self.config().upstream_ref;
            let log_spec = match target_repo.core_repo().kind() {
                vcs_core::BackendKind::Jj => {
                    // intentionally exclude the working copy commit @
                    format!("{upstream_ref}..@-")
                }
                vcs_core::BackendKind::Git => format!("{upstream_ref}..HEAD"),
                other => return Err(RegeneratePatchErrorReason::UnsupportedVcsBackend { backend: other }),
            };
            let log = target_repo
                .core_repo()
                .log(
                    &log_spec, // max=5 is sufficient because should we only have one change
                    5,
                )
                .await
                .map_err(RegeneratePatchErrorReason::Log)?;
            let _single_commit = if log.len() == 1 {
                &log[0]
            } else {
                return Err(RegeneratePatchErrorReason::NotExactlyOneCommit {
                    log_rev: log_spec,
                    count: log.len(),
                });
            };
            let diff_spec = match target_repo.vcs_kind() {
                VcsKind::Git => {
                    // this is passed directly to `git diff`,
                    // so implicitly includes the working copy changes
                    format!("^{upstream_ref}")
                }
                VcsKind::Jujutsu => {
                    format!("{upstream_ref}::@")
                }
            };
            fn build_git_opts<'a>(after: &[&'a str]) -> Vec<&'a str> {
                // see vcs_core here for some of the flags we include:
                // https://github.com/ZelAnton/vcs-toolkit-rs/blob/vcs-git-v0.11.0/crates/git/src/lib.rs#L1350-L1367
                // need to directly so we can add --no-renames and --no-prefix flags
                const GIT_SHARED_OPTS: &[&str] =
                    &["diff", "--no-color", "--no-ext-diff", "--no-renames", "--no-prefix"];
                let mut res = GIT_SHARED_OPTS.to_vec();
                res.extend(after);
                res
            }
            let diff_cmd_args = match target_repo.vcs_kind() {
                VcsKind::Git => build_git_opts(&[upstream_ref.as_str()]),
                VcsKind::Jujutsu => {
                    // far fewer options in jj diff than in git diff
                    // to workaround this we actually configure it to use git diff indirectly
                    // TODO: Less efficient than executing git diff against the backend repo
                    static TOOL_CONFIG_OPT: LazyLock<String> = LazyLock::new(|| {
                        let git_tool_opts = build_git_opts(&["--no-index", "$left", "$right"]);
                        format!(
                            "merge-tools.git.diff-args=[{}]",
                            git_tool_opts
                                .iter()
                                .map(|x| {
                                    // no need for escaping since everything is constant
                                    format!("\"{x}\"")
                                })
                                .join(",")
                        )
                    });
                    vec![
                        "diff",
                        "--config",
                        TOOL_CONFIG_OPT.as_str(),
                        "--tool",
                        "git",
                        "-r",
                        diff_spec.as_str(),
                    ]
                }
            };
            let diff_text = duct::cmd(target_repo.vcs_kind().binary_name(), &diff_cmd_args)
                .log_on_spawn(logger)
                .dir(target_repo.workdir())
                .read()
                .map_err(|cause| RegeneratePatchErrorReason::RunDiff {
                    diff_spec: diff_spec.clone(),
                    vcs_kind: target_repo.vcs_kind(),
                    cause,
                })?;
            let resolved_patch_dir =
                Utf8PathBuf::try_from(single_patch_set.patch_dir.to_path(self.engine.root_workdir()))
                    .expect("non-UTF8 path from UTF8 pieces");
            let mut seen_files = HashSet::new();
            for patch in diffy::patch_set::PatchSet::parse(&diff_text, diffy::patch_set::ParseOptions::gitdiff()) {
                let patch = patch.map_err(|cause| RegeneratePatchErrorReason::ParseVcsDiff {
                    vcs_kind: target_repo.vcs_kind(),
                    cause,
                })?;
                let relative_target_file: Utf8PathBuf = match patch.operation() {
                    FileOperation::Delete(file) | FileOperation::Create(file) => file.into(),
                    FileOperation::Modify { original, modified } if original == modified => original.into(),
                    FileOperation::Modify {
                        original: from,
                        modified: to,
                    }
                    | FileOperation::Rename { from, to }
                    | FileOperation::Copy { from, to } => {
                        let from = from.as_ref().into();
                        let to = to.as_ref().into();
                        return Err(RegeneratePatchErrorReason::RenameUnexpected {
                            vcs_kind: target_repo.vcs_kind(),
                            from,
                            to,
                        });
                    }
                };
                let patch_file = resolved_patch_dir
                    .join(&relative_target_file)
                    .with_added_extension("patch");
                debug!(
                    logger,
                    "Regenerating patch file";
                    "target_file" => relative_target_file.as_str(),
                    "patch_file" => patch_file.as_str(),
                );
                if !relative_target_file.is_relative() {
                    return Err(RegeneratePatchErrorReason::PathTargetNotRelative {
                        target_file: relative_target_file,
                    });
                }
                assert!(
                    // duplicate files should never happen with git diff
                    seen_files.insert(relative_target_file.clone()),
                    "vcs diff has unexpected output: duplicate file {relative_target_file}"
                );
                match patch.patch() {
                    PatchKind::Text(patch) => {
                        std::fs::File::create(&patch_file)
                            .and_then(|mut file| {
                                writeln!(file, "{patch}")?;
                                file.flush()
                            })
                            .map_err(|cause| RegeneratePatchErrorReason::PatchWriteFailed {
                                patch_file: patch_file.clone(),
                                cause,
                            })?;
                    }
                    PatchKind::Binary(_) => {
                        return Err(RegeneratePatchErrorReason::BinaryPatchUnsupported {
                            target_file: relative_target_file,
                        });
                    }
                }
            }
            // delete patches for files that have no diff
            self.walk_patch_dir::<RegeneratePatchErrorReason>(&resolved_patch_dir, |entry| {
                if !seen_files.contains(Utf8Path::new(entry.relative_target_file)) {
                    debug!(
                        self.logger,
                        "Removing old patch file (no longer has a diff)";
                        "target_file" => entry.relative_target_file.as_str(),
                        "patch_file" => entry.patch_file.as_str(),
                    );
                    std::fs::remove_file(entry.patch_file).map_err(|cause| {
                        RegeneratePatchErrorReason::DeletePatchFile {
                            patch_file: entry.patch_file.to_path_buf(),
                            cause,
                        }
                    })?;
                }
                Ok(())
            })?;
            Ok(())
        })
        .map_err(&create_error)
    }

    pub fn apply_patches(&self, opts: &ApplyPatchesOptions) -> Result<(), ApplyPatchError> {
        let logger = &self.logger;
        let create_error = |reason| ApplyPatchError {
            repo_name: self.state.name.clone(),
            reason: Box::new(reason),
        };
        let target_repo = if opts.init {
            self.init_repo()
                .map_err(ApplyPatchErrorReason::InitRepo)
                .map_err(&create_error)?
        } else {
            self.state
                .repo
                .get_or_try_init(|| VcsRepo::open(self.target_workdir()))
                .map_err(ApplyPatchErrorReason::OpenExistingRepo)
                .map_err(&create_error)?
        };
        let rt = VcsRepo::create_tokio_runtime()
            .map_err(ApplyPatchErrorReason::TokioInit)
            .map_err(&create_error)?;
        rt.block_on(async {
            let start_working_changes = target_repo.query_workdir_changes().await?;
            if !start_working_changes.is_empty() {
                if opts.force {
                    warn!(
                        logger, "Overriding working directory changes";
                        "changes" => &start_working_changes,
                    );
                } else {
                    return Err(ApplyPatchErrorReason::DirtyWorktree {
                        changes: start_working_changes,
                    });
                }
            }
            debug!(
                logger,
                "Checking out upstream (new_child on jj)";
                "upstream_ref" => &self.config().upstream_ref,
            );
            target_repo
                .core_repo()
                .new_child(&self.config().upstream_ref)
                .await
                .map_err(|cause| ApplyPatchErrorReason::Checkout {
                    rev: self.config().upstream_ref.clone(),
                    cause: VcsError::new(cause),
                })?;
            if self.config().patch_sets.is_empty() {
                warn!(logger, "Repository has no configured patch sets");
            }
            for patch_set in &self.config().patch_sets {
                let resolved_patch_dir =
                    Utf8PathBuf::from_path_buf(patch_set.patch_dir.to_path(self.engine.root_workdir()))
                        .expect("relative-path is always UTF8");
                info!(
                    logger,
                    "Applying patch set";
                    "patch_dir" => %patch_set.patch_dir,
                    "style" => %patch_set.style,
                    "resolved_patch_dir" => %resolved_patch_dir,
                );
                let logger = logger.new(slog::o!(
                    "patch_dir" => patch_set.patch_dir.to_string(),
                ));
                match patch_set.style {
                    PatchSetStyle::PerFile => {
                        self.apply_per_file_patchset(&logger, patch_set, target_repo, &resolved_patch_dir)
                            .await
                            .map_err(|cause| ApplyPatchErrorReason::ApplySet {
                                patch_dir: patch_set.patch_dir.to_string().into(),
                                cause,
                            })?;
                    }
                    unsupported @ PatchSetStyle::PerCommit => {
                        return Err(ApplyPatchErrorReason::UnsupportedPatchStyle { style: unsupported });
                    }
                }
                let post_patch_changes = target_repo.query_workdir_changes().await?;
                if !post_patch_changes.is_empty() {
                    warn!(
                        logger,
                        "After committing patch set, some uncommitted changes still remain";
                        "uncommitted_changes" => &post_patch_changes,
                    );
                }
            }
            Ok(())
        })
        .map_err(&create_error)
    }

    async fn apply_per_file_patchset(
        &self,
        logger: &Logger,
        patch_config: &PatchSetConfig,
        target_repo: &VcsRepo,
        resolved_patch_dir: &Utf8Path,
    ) -> Result<(), PatchSetApplicationError> {
        let _ = self;
        // list of changed relative paths to commit
        let mut changed_paths = Vec::new();
        // This could sensibly be parallelized if that becomes necessary
        self.walk_patch_dir(resolved_patch_dir, |entry| {
            changed_paths.push(entry.relative_target_file.to_owned());
            let patch_contents =
                std::fs::read(entry.patch_file).map_err(|cause| PatchSetApplicationError::ReadPatch {
                    patch_file: entry.patch_file.into(),
                    cause,
                })?;
            let patch =
                diffy::Patch::from_bytes(&patch_contents).map_err(|cause| PatchSetApplicationError::ParsePatch {
                    patch_file: entry.patch_file.into(),
                    cause,
                })?;
            let target_file_contents = match std::fs::read(entry.target_file) {
                Ok(contents) => contents,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => {
                    // use an empty vector for file not found to make adding new files easy
                    Vec::new()
                }
                Err(cause) => {
                    return Err(PatchSetApplicationError::TargetFileRead {
                        target_file: entry.target_file.into(),
                        cause,
                    });
                }
            };
            debug!(
                logger,
                "Applying per-file patch";
                "patch_file" => %entry.patch_file,
                "target_file" => %entry.target_file,
            );
            let patched_contents =
                diffy::apply_bytes(&target_file_contents, &patch).map_err(|cause| PatchSetApplicationError::Apply {
                    cause,
                    patch_file: entry.patch_file.to_owned(),
                })?;
            std::fs::write(entry.target_file, &patched_contents).map_err(|cause| {
                PatchSetApplicationError::TargetFileWrite {
                    cause,
                    target_file: entry.target_file.to_owned(),
                }
            })?;
            Ok(())
        })?;
        let commit_msg = patch_config.commit_msg.clone().unwrap_or_else(|| {
            indoc::formatdoc! {"
                [auto] Patches for {target_repo}

                Remember to regenerate patches if this commit is changed.

                Co-Authored-By: gitpatcher <gitpatcher@techcable.github.io>
                ",
                target_repo = self.state.name
            }
        });
        let commit_target_paths = changed_paths
            .iter()
            .map(RelativePathBuf::to_string)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        // when using git, we need to add the paths before committing them
        if let Some(git_repo) = target_repo.core_repo().git() {
            git_repo
                .at(target_repo.workdir().as_std_path())
                .add(&commit_target_paths)
                .await
                .map_err(vcs_core::Error::Vcs)
                .map_err(VcsError::new)
                .map_err(|cause| PatchSetApplicationError::GitAdd { cause })?;
        }
        target_repo
            .core_repo()
            .commit_paths(&commit_target_paths, &commit_msg)
            .await
            .map_err(VcsError::new)
            .map_err(|cause| PatchSetApplicationError::Commit { cause })?;
        Ok(())
    }

    /// Walk the specified patch directory,
    /// yielding all patch files.
    ///
    /// This is currently done sequentially.
    fn walk_patch_dir<E>(
        &self,
        resolved_patch_dir: &Utf8Path,
        mut func: impl FnMut(ResolvedPatchDirEntry) -> Result<(), E>,
    ) -> Result<(), E>
    where
        E: From<PatchDirWalkError>,
    {
        const FILE_KIND: &str = "patch file";
        let walker = WalkDir::new(resolved_patch_dir);
        for entry in walker {
            let entry = entry.map_err(|cause| PatchDirWalkError::WalkDir {
                patch_dir: resolved_patch_dir.to_owned(),
                cause,
            })?;
            if !entry.file_type().is_file() {
                continue;
            }
            if !entry.file_name().as_encoded_bytes().ends_with(b".patch") {
                trace!(
                    self.logger,
                    "Skipping non-patch file";
                    "skipped_path" => entry.path().display(),
                );
                continue;
            }
            let patch_file = Utf8Path::from_path(entry.path()).ok_or_else(|| PatchDirWalkError::PathNotUtf8 {
                kind: FILE_KIND,
                path: entry.path().to_owned(),
            })?;
            let relative_patch_file = entry.path().relative_to(resolved_patch_dir).map_err(|cause| {
                PatchDirWalkError::RelativePathResolution {
                    cause,
                    kind: FILE_KIND,
                    path: entry.path().to_owned(),
                    root_dir: resolved_patch_dir.into(),
                }
            })?;
            let file_name = relative_patch_file.file_name().expect("missing file name");
            let relative_target_file = relative_patch_file.with_file_name({
                file_name
                    .strip_suffix(".patch")
                    .expect("Missing suffix after we already checked")
            });
            let target_file = Utf8PathBuf::from_path_buf(relative_target_file.to_path(self.target_workdir()))
                .expect("non-UTF8 path from UTF8 pieces");
            func(ResolvedPatchDirEntry {
                file_name,
                relative_target_file: &relative_target_file,
                relative_patch_file: &relative_patch_file,
                patch_file,
                target_file: &target_file,
            })?;
        }
        Ok(())
    }
}
#[expect(dead_code, reason = "some fields not used")]
struct ResolvedPatchDirEntry<'a> {
    file_name: &'a str,
    relative_target_file: &'a RelativePath,
    relative_patch_file: &'a RelativePath,
    patch_file: &'a Utf8Path,
    target_file: &'a Utf8Path,
}

#[derive(Debug)]
struct TargetRepoState {
    name: String,
    repo: sync::OnceCell<VcsRepo>,
    config: RepoConfig,
    workdir: Utf8PathBuf,
}

#[derive(Debug)]
pub struct PatchEngine {
    logger: Logger,
    root_repo: VcsRepo,
    prefs: PreferencesConfig,
    primary_config: PrimaryConfig,
    target_repo_states: IndexMap<String, TargetRepoState>,
}
impl PatchEngine {
    pub fn init(logger: &Logger, root_repo: VcsRepo) -> Result<Self, PatchEngineInitError> {
        Self::builder(logger, root_repo).init()
    }
    pub fn builder(logger: &Logger, root_repo: VcsRepo) -> PatchEngineBuilder {
        PatchEngineBuilder {
            parent_logger: logger.clone(),
            root_repo: Some(root_repo),
            primary_config_path: None,
        }
    }
}
pub struct PatchEngineBuilder {
    parent_logger: Logger,
    root_repo: Option<VcsRepo>,
    primary_config_path: Option<PathBuf>,
}
impl PatchEngineBuilder {
    /// Where to find the primary config file.
    ///
    /// If not specified, assumed to exist at `gitpatcher.toml` in the repo root.
    /// This is not as extensive a search as the CLI currently performs.
    pub fn primary_config_path(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.primary_config_path = Some(path.as_ref().into());
        self
    }
    pub fn init(&mut self) -> Result<PatchEngine, PatchEngineInitError> {
        let root_repo = self
            .root_repo
            .take()
            .expect("A PatchEngineBuilder cannot be used twice");
        let primary_config_path = self
            .primary_config_path
            .clone()
            .unwrap_or_else(|| root_repo.workdir().join("gitpatcher.toml").into_std_path_buf());
        let primary_config = PrimaryConfig::load_from(primary_config_path.as_ref()).map_err(|cause| {
            PatchEngineInitError::PrimaryConfigLoad {
                config_path: primary_config_path.clone(),
                cause,
            }
        })?;
        let prefs = PreferencesConfig::load().map_err(PatchEngineInitError::PrefsLoad)?;
        let target_repo_states = primary_config
            .repos
            .iter()
            .map(|(name, config)| {
                (
                    name.clone(),
                    TargetRepoState {
                        repo: sync::OnceCell::new(),
                        name: name.clone(),
                        config: config.clone(),
                        workdir: root_repo.workdir().join(Utf8Path::new(
                            config.custom_path.as_deref().unwrap_or(RelativePath::new(name)),
                        )),
                    },
                )
            })
            .collect();
        Ok(PatchEngine {
            logger: self.parent_logger.new(slog::o!(
                "root_repo" => root_repo.workdir().to_string(),
                "primary_config" => primary_config_path.display().to_string(),
            )),
            root_repo,
            primary_config,
            prefs,
            target_repo_states,
        })
    }
}
impl PatchEngine {
    /// The workdir of the root repository
    #[inline]
    pub fn root_workdir(&self) -> &Utf8Path {
        self.root_repo.workdir()
    }

    /// The root repository.
    #[inline]
    pub fn root_repo(&self) -> &VcsRepo {
        &self.root_repo
    }

    /// Get the [`PrimaryConfig`] for this engine.
    pub fn primary_config(&self) -> &PrimaryConfig {
        &self.primary_config
    }

    /// Get the target repo with the specified name,
    /// or a [`NoSuchTargetRepoError`] if not present.
    pub fn target_repo(&self, name: &str) -> Result<TargetRepo<'_>, NoSuchTargetRepoError> {
        let state = self
            .target_repo_states
            .get(name)
            .ok_or_else(|| NoSuchTargetRepoError { name: name.to_owned() })?;
        Ok(TargetRepo {
            state,
            engine: self,
            logger: self.logger.new(slog::o!(
                "target_repo" => state.workdir.to_string(),
            )),
        })
    }

    /// Get the [`slog::Logger`] associated with this engine.
    ///
    /// Has additional context beyond what was passed to [`PatchEngine::init`].
    pub fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[derive(Debug, thiserror::Error)]
#[error("No target repository named {name:?}")]
#[non_exhaustive]
pub struct NoSuchTargetRepoError {
    pub name: String,
}
