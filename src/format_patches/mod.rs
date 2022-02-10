use crate::format_patches::format::{CommitMessage, InvalidCommitMessage};
use crate::utils::SimpleParser;
use git2::{Commit, DiffOptions, Error, Oid, Repository};
use slog::{info, Logger};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

mod format;

pub struct FormatOptions {
    pub diff_opts: DiffOptions,
}
impl Default for FormatOptions {
    fn default() -> Self {
        let diff_opts = DiffOptions::new();
        FormatOptions { diff_opts }
    }
}

pub struct PatchFormatter<'repo> {
    logger: Logger,
    base: Commit<'repo>,
    last_commit: Commit<'repo>,
    out_dir: PathBuf,
    opts: FormatOptions,
    target: &'repo Repository,
}
impl<'repo> PatchFormatter<'repo> {
    pub fn new(
        logger: Logger,
        out_dir: PathBuf,
        target: &'repo Repository,
        base: Commit<'repo>,
        opts: FormatOptions,
    ) -> Result<Self, PatchFormatError> {
        Ok(PatchFormatter {
            logger,
            opts,
            out_dir,
            last_commit: base.clone(),
            base,
            target,
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
        let message = CommitMessage::from_commit(&commit).map_err(|cause| {
            PatchFormatError::InvalidCommitMessage {
                cause,
                commit_id: commit.id(),
            }
        })?;
        let last_tree = self.last_commit.tree()?;
        let tree = commit.tree()?;
        let mut diff = self.target.diff_tree_to_tree(
            Some(&last_tree),
            Some(&tree),
            // TODO: Why does diff_opts need to be mutable?
            Some(&mut self.opts.diff_opts),
        )?;
        let patch_name = message.patch_file_name(index as u32 + 1);
        let patch = self.out_dir.join(&patch_name);
        let buf = diff.format_email(1, 1, &commit, None)?;
        let s = cleanup_patch(buf.as_str().unwrap()).map_err(|cause| {
            PatchFormatError::PatchCleanupError {
                cause,
                patch_file: patch.clone(),
            }
        })?;
        std::fs::write(&patch, s).map_err(|cause| PatchFormatError::PatchWriteError {
            cause,
            patch_file: patch.clone(),
        })?;
        info!(self.logger, "Generating patch: {}", patch_name);
        Ok(())
    }
}

fn cleanup_patch(s: &str) -> Result<String, CleanupPatchErr> {
    let mut result = String::new();
    let mut pushln = |line: &str| {
        result.push_str(line);
        result.push('\n');
    };
    let mut parser = SimpleParser::new(s);
    /*
     * Ensure there is one and only one newline between
     * the summary line (Subject: [PATCH]),
     * and the rest of the commit message
     */
    let subject_line = parser
        .take_until(|line| line.starts_with("Subject: [PATCH]"), &mut pushln)
        .map_err(|_| CleanupPatchErr::UnexpectedEof {
            expected: "Subject line",
        })?;
    pushln(subject_line);
    parser.skip_whitespace();
    /*
     * libgit2 generates diff stats, which we don't care about
     * Skip until we see start of diff stats `---`
     */
    let mut trailing_commit_message = String::new();
    parser
        .take_until(
            |line| line.starts_with("---"),
            |line| {
                trailing_commit_message.push_str(line);
                trailing_commit_message.push('\n');
            },
        )
        .map_err(|_| CleanupPatchErr::UnexpectedEof {
            expected: "Diff stats",
        })?;
    let trailing_commit_message = trailing_commit_message.trim();
    pushln("");
    if !trailing_commit_message.is_empty() {
        pushln(trailing_commit_message);
    }
    pushln("");
    // Ignore until we see a `diff --git a/file.txt b/file.txt` line
    let diff_line = parser
        .take_until(|line| line.starts_with("diff"), |_| {})
        .map_err(|_| CleanupPatchErr::UnexpectedEof {
            expected: "Diff line",
        })?;
    pushln(diff_line);
    // Dump all remaining lines
    while let Ok(line) = parser.pop() {
        result.push_str(line);
        result.push('\n')
    }
    Ok(result)
}
#[derive(Debug)]
pub enum CleanupPatchErr {
    UnexpectedEof {
        expected: &'static str,
    },
    InvalidLine {
        line_number: usize,
        message: &'static str,
    },
}

#[derive(Debug)]
pub enum PatchFormatError {
    InvalidCommitMessage {
        commit_id: Oid,
        cause: InvalidCommitMessage,
    },
    PatchWriteError {
        patch_file: PathBuf,
        cause: std::io::Error,
    },
    PatchCleanupError {
        patch_file: PathBuf,
        cause: CleanupPatchErr,
    },
    InternalGit(git2::Error),
}
impl Display for PatchFormatError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            PatchFormatError::InvalidCommitMessage { commit_id, cause } => {
                write!(f, "Invalid commit message for {}: {}", commit_id, cause)
            }
            PatchFormatError::PatchWriteError { patch_file, cause } => {
                write!(f, "Error writing to {}: {}", patch_file.display(), cause)
            }
            PatchFormatError::InternalGit(cause) => Display::fmt(cause, f),
            PatchFormatError::PatchCleanupError { patch_file, cause } => {
                write!(f, "Internal error cleaning patch {}:", patch_file.display())?;
                match cause {
                    CleanupPatchErr::UnexpectedEof { expected } => {
                        writeln!(f, "Unexpected EOF, expected {}", expected)
                    }
                    CleanupPatchErr::InvalidLine {
                        line_number,
                        message,
                    } => {
                        writeln!(f, "Invalid line @ {}: {}", line_number, message)
                    }
                }
            }
        }
    }
}
impl From<git2::Error> for PatchFormatError {
    fn from(cause: Error) -> Self {
        PatchFormatError::InternalGit(cause)
    }
}
