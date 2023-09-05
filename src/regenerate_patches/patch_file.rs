use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::str::FromStr;

use bstr::ByteSlice;
use camino::{Utf8Path, Utf8PathBuf};
use git2::build::CheckoutBuilder;
use git2::{Commit, DiffFormat, DiffOptions, Repository, RepositoryState};
use nom::branch::alt;
use nom::bytes::complete::{tag, take, take_until, take_while1};
use nom::character::is_hex_digit;
use nom::combinator::{opt, recognize};
use nom::sequence::tuple;
use nom::IResult;
use slog::{debug, info, trace, warn, Logger};

use crate::format_patches::{FormatOptions, PatchFormatError, PatchFormatter};
use crate::utils::RememberLast;

pub struct PatchFileSet<'a> {
    root_repo: &'a Repository,
    patch_dir: Utf8PathBuf,
    patches: Vec<PatchFile>,
}
impl<'a> PatchFileSet<'a> {
    pub fn load(target: &'a Repository, patch_dir: &Utf8Path) -> Result<Self, PatchError> {
        assert!(patch_dir.is_relative());
        let mut set = PatchFileSet {
            root_repo: target,
            patches: Vec::new(),
            patch_dir: patch_dir.into(),
        };
        set.reload_files()?;
        Ok(set)
    }
    pub fn reload_files(&mut self) -> Result<(), PatchError> {
        self.patches.clear();
        for entry in std::fs::read_dir(&self.patch_dir)? {
            let entry = entry?;
            let file_name = match entry.file_name().to_str() {
                Some(file_name) => file_name.to_string(),
                None => continue, // Ignore non-UTF8 paths
            };
            // Ignore all files that aren't patches
            if !file_name.ends_with(".patch") {
                continue;
            }
            self.patches
                .push(PatchFile::parse(&self.patch_dir, &file_name)?);
        }
        self.patches.sort_by_key(|patch| patch.index);
        Ok(())
    }

    /// Stage any changes to patch files
    ///
    /// This implicitly discards any previously staged changes to the patch files.
    /// The gitpatcher system considers the target repo to be
    /// the authoritative source of changes.
    /// As long as you keep your changes saved in that repo, you'll be fine.
    pub fn stage_changes(&mut self) -> Result<(), git2::Error> {
        let mut index = self.root_repo.index()?;
        index.add_all(
            [self.patch_dir.as_std_path()],
            git2::IndexAddOption::DEFAULT,
            None,
        )?;
        index.write()
    }
}
pub struct PatchFile {
    index: usize,
    path: Utf8PathBuf,
}
impl PatchFile {
    fn parse(parent: &Utf8Path, file_name: &str) -> Result<Self, PatchError> {
        // Must match ASCII regex `[\d]{4}-(commit_name).patch`
        if file_name.len() >= 5 && file_name.as_bytes()[4] == b'-' && file_name.ends_with(".patch")
        {
            let index =
                usize::from_str(&file_name[..4]).map_err(|_| PatchError::InvalidPatchName {
                    name: file_name.into(),
                })?;
            Ok(PatchFile {
                index,
                path: parent.join(file_name),
            })
        } else {
            Err(PatchError::InvalidPatchName {
                name: file_name.into(),
            })
        }
    }
}

#[derive(Default)]
pub struct RegenerateOptions {
    pub format_opts: FormatOptions,
}

pub fn regenerate_patches(
    base: &Commit,
    patch_set: &mut PatchFileSet,
    target: &Repository,
    logger: Logger,
    options: RegenerateOptions,
) -> Result<(), PatchError> {
    let target_name = target
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| panic!("Invalid path for target repo: {}", target.path().display()));
    info!(logger, "Formatting patches for {}", patch_set.patch_dir);
    // Remove old patches
    match target.state() {
        RepositoryState::Rebase | RepositoryState::RebaseInteractive => {
            // TODO: This assumes the rebase is being applied against `upstream`
            warn!(logger, "Rebase detected - partial save");
            let mut rebase = patch_set.root_repo.open_rebase(None)?;
            let next = rebase.operation_current().unwrap_or(0);
            for patch in &patch_set.patches[..next] {
                std::fs::remove_file(&patch.path)?;
            }
        }
        RepositoryState::Clean => {
            for patch in &patch_set.patches {
                std::fs::remove_file(&patch.path)?;
            }
        }
        state => {
            return Err(PatchError::PatchedRepoInvalidState { state });
        }
    }

    // Regenerate the patches
    {
        PatchFormatter::new(
            logger.clone(),
            patch_set.patch_dir.clone(),
            target,
            base.clone(),
            options.format_opts,
        )?
        .generate_all()?;
        patch_set.reload_files()?;
    }

    patch_set.stage_changes()?;

    // Remove any 'trivial' patches
    {
        let head_tree = patch_set.root_repo.head()?.peel_to_tree()?;
        let mut filtered_tree = None;
        let mut parents = patch_set.patch_dir.ancestors().collect::<Vec<_>>();
        let len = parents.len();
        parents.truncate(len - 1); // Trim last (empty)
        for path in parents {
            let entry = head_tree.get_path(path.as_std_path())?;
            let child_tree = match filtered_tree {
                None => {
                    let tree = entry.to_object(patch_set.root_repo)?.peel_to_tree()?;
                    // Use our initial tree which is a copy of `patch_dir` itself
                    patch_set.root_repo.treebuilder(Some(&tree))?
                }
                Some(existing_tree) => existing_tree,
            };
            let mut builder = patch_set.root_repo.treebuilder(None)?;
            builder.insert(
                path.file_name()
                    .unwrap_or_else(|| panic!("Invalid parent {:?}", path)),
                child_tree.write()?,
                entry.filemode(),
            )?;
            filtered_tree = Some(builder);
        }
        let filtered_tree = patch_set
            .root_repo
            .find_tree(filtered_tree.unwrap().write()?)?;
        let mut ops = DiffOptions::new();
        ops.ignore_whitespace_eol(true);
        let diff = patch_set
            .root_repo
            .diff_tree_to_index(Some(&filtered_tree), None, None)?;
        let mut deltas_by_path = HashMap::new();
        diff.print(DiffFormat::Patch, |delta, _hunk, line| {
            // TODO: Propagate errors instead of panicking
            let buffer = deltas_by_path
                .entry(delta.new_file().path().unwrap().to_path_buf())
                .or_insert_with(String::new);
            let origin = line.origin();
            match origin {
                ' ' | '+' | '-' => buffer.push(origin),
                _ => {}
            }
            buffer.push_str(std::str::from_utf8(line.content()).unwrap());
            true
        })?;
        let mut checkout_patches = CheckoutBuilder::new();
        checkout_patches.recreate_missing(true);
        checkout_patches.force();
        let mut num_trivial = 0;
        for patch in &patch_set.patches {
            let git_version = {
                let mut reader = BufReader::new(File::open(&patch.path)?);
                let mut remember = RememberLast::<_, 2>::new();
                let mut buffer = String::new();
                while reader.read_line(&mut buffer)? != 0 {
                    remember.remember(&buffer);
                    buffer.clear();
                }
                let last = remember.as_slice();
                if last[1].chars().all(|c| c.is_whitespace()) {
                    // If the last line is all whitespace go with the second to last line
                    &last[0]
                } else {
                    &last[1]
                }
                .trim()
                .to_string()
            };
            let delta = match deltas_by_path.get(patch.path.as_std_path()) {
                Some(delta) => delta,
                None => continue, // no delta -> no changes to checkout
            };
            let patch_logger = logger.new(slog::o!("patch" => patch.path.as_str().to_string()));
            if is_trivial_patch_change(&patch_logger, delta, &git_version) {
                debug!(patch_logger, "Ignoring trivial patch");
                num_trivial += 1;
                checkout_patches.path(patch.path.as_std_path());
            }
        }
        if num_trivial > 0 {
            patch_set
                .root_repo
                .checkout_head(Some(&mut checkout_patches))?;
        }
    }

    info!(logger, "Patches for {}", target_name);
    Ok(())
}
fn is_trivial_patch_change(logger: &Logger, diff: &str, git_ver: &str) -> bool {
    const CHANGE_MARKERS: &[char] = &['+', '-'];
    let lines = diff.lines();
    // NOTE: Remember one more than we strictly need
    let mut remember = RememberLast::<_, 5>::new();
    for (idx, line) in lines.enumerate() {
        // We only care about lines that are (+|-)
        if !line.starts_with(CHANGE_MARKERS) {
            continue;
        }
        if is_trivial_line(line.as_bytes()) {
            trace!(logger, "Ignoring 'trivial' line"; "line" => ?line, "number" => idx + 1);
        } else {
            trace!(logger, "Found non-trivial line"; "line" => ?line, "number" => idx + 1);
            // We found a non-trivial change in this patch
            remember.remember(&line);
        }
    }
    match remember.len() {
        0 => true,
        // Ignore changes to $git_ver
        1 => remember.back(0)[1..].trim() == git_ver,
        _ => {
            // Ignore changes to trailing git version info
            let mut ignored_changes = 0;
            // There could be a blank line before the change to git version
            if remember.back(0)[1..].trim().is_empty() {
                ignored_changes += 1;
            }
            if remember.back(ignored_changes)[1..].trim() == git_ver {
                ignored_changes += 1;
                /*
                 * The last change was to the git version
                 * Strip any other related changes
                 */
                if remember.len() + ignored_changes >= 2
                    && remember.back(ignored_changes)[1..].trim() == "--"
                    && remember.back(ignored_changes + 2)[1..].trim() == "--"
                {
                    // They also changed the -- at the end
                    ignored_changes += 3;
                } else {
                    // Ignore the change to the old version
                    ignored_changes += 1;
                }
            }
            assert!(ignored_changes <= remember.as_slice().len());
            ignored_changes == remember.as_slice().len()
        }
    }
}
fn is_trivial_line(line: &[u8]) -> bool {
    if line.contains_str("--- a") | line.contains_str("+++ b") {
        true
    } else {
        let res: IResult<&[u8], &[u8]> = alt((
            recognize(tuple((take_until("From "), take_while1(is_hex_digit)))),
            recognize(tuple((opt(take(1usize)), tag("index")))),
        ))(line);
        res.is_ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    /// The patched repo was in an invalid [RepositoryState]
    #[error("Target repo is in unexpected state: {state:?}")]
    PatchedRepoInvalidState { state: RepositoryState },
    #[error("Invalid name for patch: {name:?}")]
    InvalidPatchName { name: String },
    #[error("Failed to format patches: {0}")]
    PatchFormatFailed(#[from] PatchFormatError),
    #[error("Missing patch dir {}: {cause}", patch_dir)]
    MissingPatchDir {
        patch_dir: Utf8PathBuf,
        #[source]
        cause: git2::Error,
    },
    /// An unexpected error occurred using git
    #[error("Unexpected git error: {0}")]
    Git(#[from] git2::Error),
    #[error("Unexpected IO error: {0}")]
    Io(#[from] std::io::Error),
}
