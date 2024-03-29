use bstr::{BStr, BString, ByteSlice, ByteVec};
use camino::Utf8PathBuf;
use git2::{Commit, DiffOptions, EmailCreateOptions, Oid, Repository};
use slog::{error, info, Logger};

use crate::format_patches::format::{CommitMessage, InvalidCommitMessage};
use crate::utils::SimpleParser;

mod format;

pub struct FormatOptions {
    email_opts: EmailCreateOptions,
}

impl FormatOptions {
    pub fn diff_opts(&mut self) -> &mut DiffOptions {
        self.email_opts.diff_options()
    }
}
impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            email_opts: EmailCreateOptions::new(),
        }
    }
}

pub struct PatchFormatter<'repo> {
    logger: Logger,
    base: Commit<'repo>,
    last_commit: Commit<'repo>,
    out_dir: Utf8PathBuf,
    opts: FormatOptions,
    target: &'repo Repository,
}
impl<'repo> PatchFormatter<'repo> {
    pub fn new(
        logger: Logger,
        out_dir: Utf8PathBuf,
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
        let message = CommitMessage::from_commit(commit).map_err(|cause| {
            PatchFormatError::InvalidCommitMessage {
                cause,
                commit_id: commit.id(),
            }
        })?;
        let last_tree = self.last_commit.tree()?;
        let tree = commit.tree()?;
        let diff = self.target.diff_tree_to_tree(
            Some(&last_tree),
            Some(&tree),
            // TODO: Why does diff_opts need to be mutable?
            Some(self.opts.diff_opts()),
        )?;
        let patch_name = message.patch_file_name(index as u32 + 1);
        let patch = self.out_dir.join(&patch_name);
        let email = git2::Email::from_diff(
            &diff,
            /* patch_idx */ 1,
            /* patch_count */ 1,
            /* commit_id */ &commit.id(),
            /* summary */ message.summary(),
            /* body */ message.body(),
            /* author */ &commit.author(),
            &mut self.opts.email_opts,
        )?;
        let s = cleanup_patch(BStr::new(email.as_slice())).map_err(|cause| {
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

fn cleanup_patch(s: &BStr) -> Result<BString, CleanupPatchErr> {
    let mut result = BString::new(Vec::new());
    let mut pushln = |line: &BStr| {
        result.push_str(line);
        result.push_char('\n');
    };
    let mut parser = SimpleParser::new(s);
    /*
     * Ensure there is one and only one newline between
     * the summary line (Subject: [PATCH]),
     * and the rest of the commit message
     */
    let subject_line = parser
        .take_until(|line| line.starts_with(b"Subject: [PATCH]"), &mut pushln)
        .map_err(|_| CleanupPatchErr::UnexpectedEof {
            expected: "Subject line",
        })?;
    pushln(subject_line);
    parser.skip_whitespace();
    /*
     * libgit2 generates diff stats, which we don't care about
     * Skip until we see start of diff stats `---`
     */
    let mut trailing_commit_message = BString::new(Vec::new());
    parser
        .take_until(
            |line| line.starts_with(b"---"),
            |line| {
                trailing_commit_message.push_str(line);
                trailing_commit_message.push_char('\n');
            },
        )
        .map_err(|_| CleanupPatchErr::UnexpectedEof {
            expected: "Diff stats",
        })?;
    let trailing_commit_message = trailing_commit_message.trim();
    pushln(BStr::new(""));
    if !trailing_commit_message.is_empty() {
        pushln(BStr::new(trailing_commit_message));
    }
    pushln(BStr::new(""));
    // Ignore until we see a `diff --git a/file.txt b/file.txt` line
    let diff_line = parser
        .take_until(|line| line.starts_with(b"diff"), |_| {})
        .map_err(|_| CleanupPatchErr::UnexpectedEof {
            expected: "Diff line",
        })?;
    pushln(diff_line);
    // Dump all remaining lines
    while let Ok(line) = parser.pop() {
        result.push_str(line);
        result.push_char('\n')
    }
    Ok(result)
}
#[derive(Debug, thiserror::Error)]
pub enum CleanupPatchErr {
    #[error("Unexpected EOF, expected {expected}")]
    UnexpectedEof { expected: &'static str },
    #[error("Invalid line @ {line_number}: {message}")]
    InvalidLine {
        line_number: usize,
        message: &'static str,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PatchFormatError {
    #[error("Invalid commit message for {}: {}", commit_id, cause)]
    InvalidCommitMessage {
        commit_id: Oid,
        #[source]
        cause: InvalidCommitMessage,
    },
    #[error("Error writing to {patch_file}: {cause}")]
    PatchWriteError {
        patch_file: Utf8PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("Internal error cleaning patch {patch_file}: {cause}")]
    PatchCleanupError {
        patch_file: Utf8PathBuf,
        cause: CleanupPatchErr,
    },
    #[error(transparent)]
    PathNotUtf8(#[from] camino::FromPathBufError),
    #[error("Internal git error: {0}")]
    InternalGit(#[from] git2::Error),
}
