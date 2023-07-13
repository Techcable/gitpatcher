use bstr::ByteSlice;
use std::fmt::{self, Display};
use std::path::{Path, PathBuf};

use git2::{ApplyLocation, Delta as DeltaStatus, Repository, Signature};
use nom::bytes::complete::{tag, take_until, take_until1, take_while1, take_while_m_n};
use nom::character::{is_digit, is_hex_digit};
use nom::combinator::{all_consuming, opt, recognize, rest};
use nom::{sequence::tuple, IResult};
use time::OffsetDateTime;

pub struct EmailMessage {
    date: OffsetDateTime,
    message_summary: String,
    message_tail: String,
    author_name: String,
    author_email: String,
    git_diff: git2::Diff<'static>,
}

fn parse_header_line(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let (input, (_, sha, _)) = tuple((
        tag(b"From "),
        take_while_m_n(1, 40, is_hex_digit),
        tag(b" Mon Sep 17 00:00:00 2001"),
    ))(input)?;
    Ok((input, sha))
}
struct AuthorInfo<T> {
    name: T,
    email: T,
}
impl<T> AuthorInfo<T> {
    #[inline]
    fn try_map<U, E>(self, mut func: impl FnMut(T) -> Result<U, E>) -> Result<AuthorInfo<U>, E> {
        Ok(AuthorInfo {
            name: func(self.name)?,
            email: func(self.email)?,
        })
    }
    #[inline]
    fn map<U>(self, mut func: impl FnMut(T) -> U) -> AuthorInfo<U> {
        AuthorInfo {
            name: func(self.name),
            email: func(self.email),
        }
    }
}
fn parse_author_line(input: &[u8]) -> IResult<&[u8], AuthorInfo<&[u8]>> {
    let (input, (_, name, _, email, _)) = tuple((
        tag("From: "),
        take_until1(" <"),
        tag(" <"),
        take_until1(">"),
        tag(">"),
    ))(input)?;
    Ok((input, AuthorInfo { name, email }))
}
fn parse_date_line(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let (input, (_, date)) = tuple((
        tag("Date: "),
        recognize(tuple((
            take_while1(|c| !matches!(c, b'+' | b'-')),
            nom::character::complete::one_of("+-"),
            take_while1(is_digit),
        ))),
    ))(input)?;
    Ok((input, date))
}

fn parse_subject_line(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let (input, (_, _, subject)) = tuple((tag("Subject: "), opt(tag("[PATCH] ")), rest))(input)?;
    Ok((input, subject))
}

fn parse_begin_diff_line(input: &[u8]) -> IResult<&[u8], (&[u8], &[u8])> {
    let (input, (_, file_a, _, file_b)) =
        tuple((tag("diff --git a/"), take_until(" b/"), tag(" b/"), rest))(input)?;
    Ok((input, (file_a, file_b)))
}

fn match_header_line<'a, T: 'a>(
    lines: &mut dyn Iterator<Item = &'a str>,
    expected: &'static str,
    parse_func: fn(&'a [u8]) -> IResult<&'a [u8], T>,
) -> Result<T, InvalidEmailMessage> {
    let line = lines
        .next()
        .ok_or(InvalidEmailMessage::UnexpectedEof { expected })?;
    match all_consuming(parse_func)(line.as_bytes()) {
        Ok((remaining, value)) => {
            assert_eq!(remaining, b"");
            Ok(value)
        }
        Err(nom::Err::Error(err)) | Err(nom::Err::Failure(err)) => {
            Err(InvalidEmailMessage::InvalidHeader {
                actual: line.into(),
                expected,
                reason: nom::error::Error {
                    input: std::str::from_utf8(err.input)?.into(),
                    code: err.code,
                },
            })
        }
        Err(nom::Err::Incomplete(_)) => unreachable!(),
    }
}
impl EmailMessage {
    // TODO: Accept bstr?
    pub fn parse(msg: &str) -> Result<Self, InvalidEmailMessage> {
        let git_diff = git2::Diff::from_buffer(msg.as_bytes())?;

        let mut lines = msg.lines().peekable();
        match_header_line(&mut lines, "header", parse_header_line)?;
        let author = match_header_line(&mut lines, "author", parse_author_line)?
            .try_map(std::str::from_utf8)?
            .map(String::from);
        let date = std::str::from_utf8(match_header_line(&mut lines, "date", parse_date_line)?)?;
        let message_summary = std::str::from_utf8(match_header_line(
            &mut lines,
            "subject",
            parse_subject_line,
        )?)?;
        let mut message_summary = String::from(message_summary);
        loop {
            let line = lines.next().ok_or(InvalidEmailMessage::UnexpectedEof {
                expected: "diff after subject",
            })?;
            if line.is_empty() {
                break;
            } else {
                // Breaking over newlines doesn't affect final result
                message_summary.push_str(line);
            }
        }
        /*
         * We already skipped a single line of whitespace
         * There could be several lines of `message_tail`,
         * than a single line,
         * than `git diff -a/{some_file} b/{some_file}`
         */
        let mut trailing_message = String::new();
        loop {
            let line = lines.next().ok_or(InvalidEmailMessage::UnexpectedEof {
                expected: "diff after message",
            })?;
            if line.is_empty() {
                match lines.peek() {
                    Some(line) if parse_begin_diff_line(line.as_bytes()).is_ok() => break,
                    _ => {
                        trailing_message.push('\n');
                        // NOTE: None is implicitly handled by error in next iteration
                        continue;
                    }
                }
            } else {
                trailing_message.push_str(line);
                trailing_message.push('\n');
            }
        }
        if trailing_message.ends_with('\n') {
            assert_eq!(trailing_message.pop(), Some('\n'));
        }
        let date = OffsetDateTime::parse(date, &time::format_description::well_known::Rfc2822)
            .map_err(|cause| InvalidEmailMessage::InvalidDate {
                cause,
                actual: date.into(),
            })?;
        Ok(EmailMessage {
            git_diff,
            date,
            message_summary,
            message_tail: trailing_message,
            author_name: author.name,
            author_email: author.email,
        })
    }

    pub fn full_message(&self) -> String {
        let mut message = self.message_summary.clone();
        if !self.message_tail.is_empty() {
            message.push('\n');
            message.push('\n');
            message.push_str(&self.message_tail);
        }
        message
    }

    /// Apply this email as a new commit against the repo
    pub fn apply_commit(&self, target: &Repository) -> Result<(), PatchApplyError> {
        let working_directory = target.workdir().expect("Missing working directory!");
        let mut index = target.index()?;
        const INDEX_STAGE: i32 = 0; // not sure what this does

        'deltaLoop: for (delta_idx, git_delta) in self.git_diff.deltas().enumerate() {
            let create_desc = || DeltaDesc::from_git(Some(delta_idx), git_delta);
            let check_relative = |pth: &'a Path| {
                if pth.is_relative() {
                    Ok(pth)
                } else {
                    Err(PatchApplyError::ForbiddenAbsolutePath {
                        path: pth.into(),
                        delta: create_desc(),
                    })
                }
            };
            match git_delta.status() {
                DeltaStatus::Deleted => {
                    assert!(git_delta.old_file().exists(), "Old file should exist");
                    assert!(!git_delta.new_file().exists(), "New file should not exist");
                    let path = check_relative(
                        git_delta
                            .old_file()
                            .path()
                            .expect("Old file should have path"),
                    )?;
                    index
                        .remove_path(path)
                        .map_err(|cause| PatchApplyError::DeleteFileFailed {
                            cause,
                            delta: create_desc(),
                        })?;
                    continue 'deltaLoop;
                }
                DeltaStatus::Added
                | DeltaStatus::Modified
                | DeltaStatus::Renamed
                | DeltaStatus::Copied => {
                    // fallthrough to generic handler
                }
                // unexpected status
                DeltaStatus::Unmodified
                | DeltaStatus::Ignored
                | DeltaStatus::Untracked
                | DeltaStatus::Typechange
                | DeltaStatus::Unreadable
                | DeltaStatus::Conflicted => {
                    return Err(PatchApplyError::UnexpectedDeltaStatus {
                        delta: create_desc(),
                    })
                }
            }
            let mut patch =
                git2::Patch::from_diff(&self.git_diff, delta_idx)?.ok_or_else(|| {
                    assert!(
                        git_delta.old_file().is_binary() || git_delta.new_file().is_binary(),
                        "Binary diff should be only reason for `None` ({git_delta:#?})",
                    );
                    PatchApplyError::BinaryDelta {
                        delta: create_desc(),
                    }
                })?;
            let patch_buf = patch.to_buf()?;

            let diffy_patch = diffy::Patch::from_bytes(patch_buf.as_bytes()).map_err(|cause| {
                PatchApplyError::FailParseGitDelta {
                    delta: create_desc(),
                    cause,
                }
            })?;
            let existing: Option<(git2::IndexEntry, git2::Blob)> = match git_delta.old_file().path()
            {
                None => None,
                Some(old_path) => {
                    check_relative(old_path)?;
                    // Read bytes from the index
                    let entry = index.get_path(old_path, INDEX_STAGE).ok_or_else(|| {
                        PatchApplyError::MissingOriginalFile {
                            path: old_path.into(),
                            delta: create_desc(),
                        }
                    })?;
                    let blob = target.find_blob(entry.id)?;
                    assert_eq!(blob.id(), entry.id);
                    Some((entry, blob))
                }
            };
            let existing_bytes: &[u8] = existing.as_ref().map_or(b"", |(_, blob)| blob.content());
            let patched_bytes =
                diffy::apply_bytes(existing_bytes, &diffy_patch).map_err(|cause| {
                    PatchApplyError::FailApplyDelta {
                        delta: create_desc(),
                        cause,
                    }
                })?;
        }
        target.apply(&self.git_diff, ApplyLocation::Both, None)?;
        let time = git2::Time::new(
            self.date.unix_timestamp(),
            // seconds -> minutes
            self.date.offset().whole_minutes() as i32,
        );
        let author = Signature::new(&self.author_name, &self.author_email, &time)?;
        let tree = target.index()?.write_tree_to(target)?;
        let tree = target.find_tree(tree)?;
        // TODO: Handle detatched head/no commits
        let head_commit = target.head()?.peel_to_commit()?;
        let parents = vec![&head_commit];
        let message = self.full_message();
        target.commit(Some("HEAD"), &author, &author, &message, &tree, &parents)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidEmailMessage {
    #[error("Unexpected EOF, expected {expected}")]
    UnexpectedEof { expected: &'static str },
    #[error("Invalid header line, expected {expected}: {actual:?}")]
    InvalidHeader {
        expected: &'static str,
        actual: String,
        #[source]
        reason: nom::error::Error<String>,
    },
    #[error("Invalid date {actual:?}: {cause}")]
    InvalidDate {
        actual: String,
        #[source]
        cause: time::error::Parse,
    },
    #[error("Invalid UTF8")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("Internal git error: {0}")]
    Git(#[from] git2::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum PatchApplyError {
    #[error("Deleting file failed for delta `{delta}`, {cause}")]
    DeleteFileFailed {
        delta: DeltaDesc,
        #[source]
        cause: git2::Error,
    },
    #[error("Missing original file for {delta}")]
    MissingOriginalFile { delta: DeltaDesc, path: PathBuf },
    #[error("Absolute paths are forbidden: `{path}` (in delta {delta})", path = path.display())]
    ForbiddenAbsolutePath { delta: DeltaDesc, path: PathBuf },
    #[error("Unexpected status for delta: {delta}")]
    UnexpectedDeltaStatus { delta: DeltaDesc },
    #[error("Unexpected binary delta {delta}")]
    BinaryDelta { delta: DeltaDesc },
    #[error("Diffy failed to parse git delta {delta}, {cause}")]
    FailParseGitDelta {
        delta: DeltaDesc,
        #[source]
        cause: diffy::ParsePatchError,
    },
    #[error("Failed to apply delta {delta}, {cause}")]
    FailApplyDelta {
        delta: DeltaDesc,
        #[source]
        cause: diffy::ApplyError,
    },
    #[error("Internal git error: {cause}")]
    Git {
        #[from]
        cause: git2::Error,
        #[cfg(backtrace)]
        #[backtrace]
        backtrace: std::backtrace::Backtrace,
    },
}

fn delta_status_name(status: DeltaStatus) -> &'static str {
    match status {
        DeltaStatus::Unmodified => "unmodified",
        DeltaStatus::Added => "added",
        DeltaStatus::Deleted => "deleted",
        DeltaStatus::Modified => "modified",
        DeltaStatus::Renamed => "renamed",
        DeltaStatus::Copied => "copied",
        DeltaStatus::Ignored => "ignored",
        DeltaStatus::Untracked => "untracked",
        DeltaStatus::Typechange => "type changed",
        DeltaStatus::Unreadable => "unreadable",
        DeltaStatus::Conflicted => "conflicted",
    }
}

#[derive(Debug)]
pub struct DeltaDesc {
    delta_index: Option<usize>,
    old_file: DeltaFileDesc,
    new_file: DeltaFileDesc,
    delta_status: git2::Delta,
}

impl DeltaDesc {
    fn from_git(index: Option<usize>, git_delta: git2::DiffDelta) -> Self {
        let old_file = DeltaFileDesc::from(git_delta.old_file());
        let new_file = DeltaFileDesc::from(git_delta.new_file());
        match git_delta.status() {
            DeltaStatus::Added => {
                assert_eq!(old_file.path, None);
                assert_ne!(new_file.path, None);
            }
            DeltaStatus::Deleted => {
                assert_ne!(old_file.path, None);
                assert_eq!(new_file.path, None);
            }
            _ => {}
        }
        DeltaDesc {
            old_file,
            new_file,
            delta_status: git_delta.status(),
            delta_index: index,
        }
    }
}
impl Display for DeltaDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.delta_status {
            DeltaStatus::Added => write!(f, "added {new_file}", new_file = self.new_file)?,
            DeltaStatus::Deleted => write!(f, "removed {old_file}", old_file = self.old_file)?,
            _ => {
                write!(
                    f,
                    "{status_name} {old_file}",
                    status_name = delta_status_name(self.delta_status),
                    old_file = self.old_file
                )?;
                if self.old_file.path != self.new_file.path {
                    write!(f, " -> {new_file}", new_file = self.new_file)?;
                }
            }
        }
        if let Some(index) = self.delta_index {
            write!(f, " (#{})", index + 1)?;
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct DeltaFileDesc {
    path: Option<PathBuf>,
    oid: git2::Oid,
    binary: bool,
}
impl Display for DeltaFileDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.path {
            Some(ref path) => {
                if self.binary {
                    f.write_str("binary ")?;
                }
                Display::fmt(&path.display(), f)
            }
            None => f.write_str("/dev/null"),
        }
    }
}

impl<'a> From<git2::DiffFile<'a>> for DeltaFileDesc {
    fn from(git_file: git2::DiffFile<'a>) -> Self {
        DeltaFileDesc {
            path: if git_file.exists() {
                Some(
                    git_file
                        .path()
                        .expect("diff file exists, but path is None")
                        .into(),
                )
            } else {
                assert_eq!(git_file.path(), None, "null path for non-existent file");
                None
            },
            binary: git_file.is_binary(),
            oid: git_file.id(),
        }
    }
}
