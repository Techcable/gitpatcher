use git2::{Commit, Oid, Error, DiffOptions, Repository};
use std::path::PathBuf;
use crate::format_patches::format::{CommitMessage, InvalidCommitMessage};
use std::fmt::{Display, Formatter};
use std::fmt;
use slog::{Logger, debug};

mod format;

pub struct FormatOptions {
    pub diff_opts: DiffOptions
}
impl Default for FormatOptions {
    fn default() -> Self {
        let mut diff_opts = DiffOptions::new();
        diff_opts.ignore_whitespace_eol(true);
        diff_opts.ignore_whitespace_change(true);
        FormatOptions { diff_opts }
    }
}

pub struct PatchFormatter<'repo> {
    logger: Logger,
    base: Commit<'repo>,
    last_commit: Commit<'repo>,
    out_dir: PathBuf,
    opts: FormatOptions,
    target: &'repo Repository
}
impl<'repo> PatchFormatter<'repo> {
    pub fn new(logger: Logger, out_dir: PathBuf, target: &'repo Repository, base: Commit<'repo>, opts: FormatOptions)
               -> Result<Self, PatchFormatError> {
        Ok(PatchFormatter {
            logger, opts, out_dir, last_commit: base.clone(),
            base, target,
        })
    }
    pub fn generate_all(&mut self) -> Result<(), PatchFormatError> {
        // Walk all commits from [base]->HEAD
        let mut revwalk = self.target.revwalk()?;
        revwalk.hide(self.base.id())?;
        revwalk.push_head()?;
        revwalk.set_sorting(git2::Sort::REVERSE | git2::Sort::TOPOLOGICAL)?;
        for (index, oid) in revwalk.enumerate() {
            let commit = self.target.find_commit(oid?)?;
            self.generate(index, &commit)?;
            self.last_commit = commit;
        }
        Ok(())
    }
    fn generate(&mut self, index: usize, commit: &Commit<'repo>) -> Result<(), PatchFormatError> {
        let message = CommitMessage::from_commit(&commit)
            .map_err(|cause| PatchFormatError::InvalidCommitMessage {
                cause, commit_id: commit.id()
            })?;
        let last_tree = self.last_commit.tree()?;
        let tree = commit.tree()?;
        let mut diff = self.target.diff_tree_to_tree(
            Some(&last_tree),
            Some(&tree),
            // TODO: Why does diff_opts need to be mutable?
            Some(&mut self.opts.diff_opts)
        )?;
        let patch_name = message.patch_file_name(index as u32 + 1);
        let patch = self.out_dir.join(&patch_name);
        let buf = diff.format_email(1, 1, &commit, None)?;
        std::fs::write(&patch, &*buf).map_err(|cause| PatchFormatError::PatchWriteError {
            cause, patch_file: patch.clone()
        })?;
        debug!(self.logger, "Generating patch: {}", patch_name);
        Ok(())
    }
}

#[derive(Debug)]
pub enum PatchFormatError {
    InvalidCommitMessage {
        commit_id: Oid,
        cause: InvalidCommitMessage
    },
    PatchWriteError {
        patch_file: PathBuf,
        cause: std::io::Error
    },
    InternalGit(git2::Error)
}
impl Display for PatchFormatError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            PatchFormatError::InvalidCommitMessage { commit_id, cause } => {
                write!(f, "Invalid commit message for {}: {}", commit_id, cause)
            },
            PatchFormatError::PatchWriteError { patch_file, cause } => {
                write!(f, "Error writing to {}: {}", patch_file.display(), cause)
            },
            PatchFormatError::InternalGit(cause) => {
                Display::fmt(cause, f)
            },
        }
    }
}
impl From<git2::Error> for PatchFormatError {
    fn from(cause: Error) -> Self {
        PatchFormatError::InternalGit(cause)
    }
}