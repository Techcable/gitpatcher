use git2::{Repository, RepositoryState, Oid, DiffOptions, DiffFormat, Diff, Commit, ResetType};
use std::path::{Path, PathBuf};
use std::io::{Write, BufWriter};
use slog::{Logger, info, debug, warn};
use std::str::FromStr;
use chrono::TimeZone;
use regex::bytes::Regex;
use lazy_static::lazy_static;
use git2::build::CheckoutBuilder;
use std::fs::File;

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

pub struct RegenerateOptions {
    pub diff_opts: DiffOptions
}
impl Default for RegenerateOptions {
    fn default() -> RegenerateOptions {
        let mut diff_opts = DiffOptions::new();
        diff_opts.ignore_whitespace_eol(true);
        diff_opts.ignore_whitespace_change(true);
        RegenerateOptions { diff_opts }
    }
}

pub fn regenerate_patches(
    base: &Commit,
    original_patches: &PatchFileSet,
    target: &Repository,
    logger: Logger,
    mut options: RegenerateOptions
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
        // Walk all commits from [base]->HEAD
        let mut revwalk = target.revwalk()?;
        revwalk.hide(base.id())?;
        revwalk.push_head()?;
        revwalk.set_sorting(git2::Sort::REVERSE | git2::Sort::TOPOLOGICAL)?;
        let mut last_commit = base.clone();
        for (index, oid) in revwalk.enumerate() {
            let commit = target.find_commit(oid?)?;
            let commit_summary = commit.summary()
                .ok_or_else(|| PatchError::InvalidCommitSummary { id: commit.id(), name: None })?;
            if !commit_summary.bytes()
                .all(|b| b == b' ' || b.is_ascii_alphanumeric() || b.is_ascii_punctuation()) {
                return Err(PatchError::InvalidCommitSummary {
                    id: commit.id(),
                    name: Some(commit_summary.into())
                })
            }
            let last_tree = last_commit.tree()?;
            let tree = commit.tree()?;
            let diff = target.diff_tree_to_tree(
                Some(&last_tree),
                Some(&tree),
                // TODO: Why does diff_opts need to be mutable?
                Some(&mut options.diff_opts)
            )?;
            let patch_name = patch_file_name(commit_summary, index);
            let patch = original_patches.patch_dir.join(&patch_name);
            let mut writer = BufWriter::new(File::create(&patch)?);
            debug!(logger, "Generating patch: {}", patch_name);
            write_header(&commit, commit_summary, &mut writer)?;
            write_diff(&diff, &mut writer)?;
            last_commit = commit;
        }
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
fn patch_file_name(commit_summary: &str, index: usize) -> String {
    let mut commit_summary = commit_summary
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "-");
    commit_summary.truncate(52);
    let commit_summary = commit_summary.trim_end_matches("-");
    format!(
        "{:04}-{}.patch",
        index + 1,
        commit_summary
    )
}

fn format_time(time: git2::Time) -> String {
    let timestamp = chrono::Utc.timestamp(time.seconds(), 0);
    let mut buffer = timestamp.format("%a, %-d %b %Y %T").to_string();
    buffer.push(' ');
    use std::fmt::Write;
    write!(
        &mut buffer, "{}{:04}",
        if time.offset_minutes() < 0 { '-' } else { '+' },
        time.offset_minutes().abs()
    ).unwrap();
    buffer
}

fn write_header<T: Write>(commit: &Commit, commit_summary: &str, mut out: T) -> Result<(), PatchError> {
    writeln!(out, "From {} Mon Sep 17 00:00:00 2001", commit.id())?;
    writeln!(
        out, "From: {} <{}>",
        commit.author().name().unwrap(),
        commit.author().email().unwrap()
    )?;
    writeln!(out, "Date: {}", format_time(commit.author().when()))?;
    writeln!(out, "Subject: [PATCH] {}", commit_summary)?;
    writeln!(out)?;
    let mut stripped_message = commit.message()
        .ok_or_else(|| PatchError::InvalidCommitMessage {
            id: commit.id(), summary: commit_summary.into()
        })?;
    stripped_message = stripped_message.trim_start();
    assert!(stripped_message.starts_with(commit_summary));
    stripped_message = &stripped_message[commit_summary.len()..];
    stripped_message = stripped_message.trim();
    for msg in stripped_message.lines() {
        writeln!(out, "{}", msg)?;
    }
    writeln!(out)?;
    Ok(())
}
fn write_diff<T: Write>(delta: &Diff, mut out: T) -> Result<(), PatchError> {
    let mut io_error = None;
    let result = delta.print(
        DiffFormat::Patch,
        |_delta, _hunk, line| {
            let c = line.origin();
            match c {
                '+' | '-' | ' ' => {
                    // We actually need this
                    if let Err(e) = write!(out, "{}", c) {
                        io_error = Some(e);
                        return false;
                    }
                },
                _ => {}
            }
            if let Err(e) = out.write(line.content()) {
                io_error = Some(e);
                return false;
            }
            true // continue
        }
    );
    // Check if the closure gave an IoError
    if let Some(cause) = io_error {
        return Err(cause.into())
    }
    result?;
    Ok(())
}

#[derive(Debug)]
pub enum PatchError {
    /// The patched repo was in an invalid [RepositoryState]
    PatchedRepoInvalidState {
        state: RepositoryState
    },
    InvalidCommitSummary {
        id: Oid,
        name: Option<String>
    },
    InvalidCommitMessage {
        id: Oid,
        summary: String
    },
    InvalidPatchName {
        name: String
    },
    /// An unexpected error occurred using git
    Git(git2::Error),
    Io(std::io::Error)
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
            PatchError::InvalidCommitSummary { id, name } => {
                write!(f, "Invalid commit summary for {}: ", id)?;
                match name {
                    None => write!(f, "<unknown>"),
                    Some(s) => write!(f, "{}", s),
                }
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
            PatchError::InvalidCommitMessage { id, summary: _ } => {
                write!(f, "Invalid commit message for {}", id)
            }
        }
    }
}