use bstr::ByteSlice;
use camino::{Utf8Path, Utf8PathBuf};
use std::fmt::{self, Display};

use git2::build::TreeUpdateBuilder;
use git2::{Delta as DeltaStatus, FileMode, Repository, ResetType, Signature};
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
}

struct DeltaApplyContext<'repo, 'tree, 'builder> {
    repo: &'repo git2::Repository,
    delta_idx: usize,
    git_delta: git2::DiffDelta<'repo>,
    desc: DeltaDesc,
    orig_tree: &'tree git2::Tree<'repo>,
    result_tree: &'builder mut TreeUpdateBuilder,
}
impl EmailMessage {
    fn apply_delta(&self, ctx: DeltaApplyContext) -> Result<(), DeltaApplyError> {
        match ctx.git_delta.status() {
            DeltaStatus::Deleted => {
                assert!(ctx.git_delta.old_file().exists(), "Old file should exist");
                assert!(
                    !ctx.git_delta.new_file().exists(),
                    "New file should not exist"
                );
                let path = ctx.desc.old_path().expect("Old file should have path");
                ctx.result_tree.remove(path.as_std_path().to_path_buf());
                return Ok(());
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
                return Err(DeltaApplyError::UnexpectedDeltaStatus {
                    status: ctx.git_delta.status(),
                })
            }
        }
        let mut patch = git2::Patch::from_diff(&self.git_diff, ctx.delta_idx)
            .unexpected()?
            .ok_or_else(|| {
                assert!(
                    ctx.git_delta.old_file().is_binary() || ctx.git_delta.new_file().is_binary(),
                    "Binary diff should be only reason for `None` ({git_delta:#?})",
                    git_delta = &ctx.git_delta
                );
                DeltaApplyError::BinaryDelta
            })?;
        let patch_buf = patch.to_buf().unexpected()?;

        let diffy_patch = diffy::Patch::from_bytes(patch_buf.as_bytes())
            .map_err(|cause| DeltaApplyError::FailParseGitDelta { cause })?;
        let existing: Option<(git2::TreeEntry, git2::Blob)> = match ctx
            .git_delta
            .old_file()
            .path()
            /*
             * NOTE: Sometimes DeltaStatus::Added has an old_file (instead of None).
             * We need to explicitly ignore that case,
             * because otherwise the file will be missing.
             */
            .filter(|_| !matches!(ctx.git_delta.status(), DeltaStatus::Added))
        {
            None => None,
            Some(old_path) => {
                // Read bytes from the tree
                let entry = ctx.orig_tree.get_path(old_path).map_err(|_cause| {
                    DeltaApplyError::MissingOriginalFile {
                        path: old_path.into(),
                    }
                })?;
                let blob = ctx.repo.find_blob(entry.id()).unexpected()?;
                assert_eq!(blob.id(), entry.id());
                Some((entry, blob))
            }
        };
        let existing_bytes: &[u8] = existing.as_ref().map_or(b"", |(_, blob)| blob.content());
        let patched_bytes = diffy::apply_bytes(existing_bytes, &diffy_patch)
            .map_err(|cause| DeltaApplyError::FailApplyPatch { cause })?;
        let patched_oid = ctx.repo.blob(&patched_bytes).unexpected()?;
        ctx.result_tree.upsert(
            ctx.desc.new_path().unwrap().as_std_path(),
            patched_oid,
            FileMode::Blob,
        );
        Ok(())
    }

    /// Apply this email as a new commit against the repo
    pub fn apply_commit(&self, target: &Repository) -> Result<(), PatchApplyError> {
        let tree = target.index()?.write_tree_to(target)?;
        let tree = target.find_tree(tree)?;
        let mut new_tree = TreeUpdateBuilder::new();
        for (delta_idx, git_delta) in self.git_diff.deltas().enumerate() {
            let desc = DeltaDesc::from_git(Some(delta_idx), &git_delta)?;
            self.apply_delta(DeltaApplyContext {
                git_delta,
                delta_idx,
                orig_tree: &tree,
                repo: target,
                desc: desc.clone(),
                result_tree: &mut new_tree,
            })
            .map_err(|cause| PatchApplyError::FailDelta {
                cause,
                delta: Box::new(desc.clone()),
            })?
        }
        let updated_tree_oid = new_tree
            .create_updated(target, &tree)
            .map_err(|cause| PatchApplyError::FailBuildTree { cause })?;
        let updated_tree = target.find_tree(updated_tree_oid).unexpected()?;
        // target.apply(&self.git_diff, ApplyLocation::Both, None)?;
        let time = git2::Time::new(
            self.date.unix_timestamp(),
            // seconds -> minutes
            self.date.offset().whole_minutes() as i32,
        );
        let author = Signature::new(&self.author_name, &self.author_email, &time)?;
        // TODO: Handle detatched head/no commits
        let head_commit = target.head()?.peel_to_commit()?;
        let parents = vec![&head_commit];
        let message = self.full_message();
        let commit_id = target.commit(
            Some("HEAD"),
            &author,
            &author,
            &message,
            &updated_tree,
            &parents,
        )?;
        let commit = target.find_commit(commit_id).unexpected()?;
        target
            .reset(commit.as_object(), ResetType::Hard, None)
            .unexpected()?;
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
    #[error("Failed to apply delta {delta}")]
    FailDelta {
        delta: Box<DeltaDesc>,
        #[source]
        cause: DeltaApplyError,
    },
    #[error("Failed to construct updated tree")]
    FailBuildTree {
        #[source]
        cause: git2::Error,
    },
    #[error(transparent)]
    ForbiddenAbsolutePath(#[from] AbsolutePathError),
    #[error(transparent)]
    InvalidUtf8Path(#[from] camino::FromPathBufError),
    #[error("Internal git error: {cause}")]
    UnexpectedGit {
        #[from]
        cause: git2::Error,
        #[cfg(backtrace)]
        #[backtrace]
        backtrace: std::backtrace::Backtrace,
    },
}
impl From<BadPathError> for PatchApplyError {
    fn from(value: BadPathError) -> Self {
        match value {
            BadPathError::AbsolutePath(cause) => cause.into(),
            BadPathError::InvalidUtf8Path(cause) => cause.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeltaApplyError {
    #[error("Deleting file failed, {cause}")]
    DeleteFileFailed {
        #[source]
        cause: git2::Error,
    },
    #[error("Missing original file")]
    MissingOriginalFile { path: std::path::PathBuf },
    #[error("Unexpected delta status")]
    UnexpectedDeltaStatus { status: DeltaStatus },
    #[error("Unexpected binary delta")]
    BinaryDelta,
    #[error("Diffy failed to parse git delta, {cause}")]
    FailParseGitDelta {
        #[source]
        cause: diffy::ParsePatchError,
    },
    #[error("Diffy failed to apply patch, {cause}")]
    FailApplyPatch {
        #[source]
        cause: diffy::ApplyError,
    },
    #[error("Internal git error: {cause}")]
    UnexpectedGit {
        #[source]
        cause: git2::Error,
        #[cfg(backtrace)]
        #[backtrace]
        backtrace: std::backtrace::Backtrace,
    },
}
struct UnexpectedGitError {
    cause: git2::Error,
    #[cfg(backtrace)]
    backtrace: std::backtrace::Backtrace,
}
impl From<UnexpectedGitError> for DeltaApplyError {
    #[inline]
    fn from(value: UnexpectedGitError) -> Self {
        DeltaApplyError::UnexpectedGit {
            cause: value.cause,
            #[cfg(backtrace)]
            backtrace: value.backtrace,
        }
    }
}
impl From<UnexpectedGitError> for PatchApplyError {
    fn from(value: UnexpectedGitError) -> Self {
        PatchApplyError::UnexpectedGit {
            cause: value.cause,
            #[cfg(backtrace)]
            backtrace: value.backtrace,
        }
    }
}
trait IntoUnexpected {
    type Res: Sized;
    fn unexpected(self) -> Self::Res;
}
impl IntoUnexpected for git2::Error {
    type Res = UnexpectedGitError;
    #[cold]
    #[inline(always)] // don't want this in backtraces
    fn unexpected(self) -> UnexpectedGitError {
        UnexpectedGitError {
            cause: self,
            #[cfg(backtrace)]
            backtrace: std::backtrace::Backtrace::capture(),
        }
    }
}
impl<T, E> IntoUnexpected for Result<T, E>
where
    E: IntoUnexpected,
{
    type Res = Result<T, E::Res>;
    #[cold]
    #[inline(always)] // don't want this in backtraces
    fn unexpected(self) -> Result<T, E::Res> {
        match self {
            Ok(value) => Ok(value),
            Err(err) => Err(err.unexpected()),
        }
    }
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

#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "Absolute paths are forbidden for {role} (path `{path}`)",
    role = role.unwrap_or("?")
)]
pub struct AbsolutePathError {
    path: Utf8PathBuf,
    role: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct DeltaDesc {
    delta_index: Option<usize>,
    old_file: DeltaFileDesc,
    new_file: DeltaFileDesc,
    delta_status: git2::Delta,
}

impl DeltaDesc {
    fn old_path(&self) -> Option<&Utf8Path> {
        self.old_file.path.as_deref()
    }
    fn new_path(&self) -> Option<&Utf8Path> {
        self.new_file.path.as_deref()
    }
    fn from_git(index: Option<usize>, git_delta: &git2::DiffDelta) -> Result<Self, BadPathError> {
        let old_file = DeltaFileDesc::try_from(git_delta.old_file()).map_err(|mut err| {
            err.set_role("old_file");
            err
        })?;
        let new_file = DeltaFileDesc::try_from(git_delta.new_file()).map_err(|mut err| {
            err.set_role("new_file");
            err
        })?;
        Ok(DeltaDesc {
            old_file,
            new_file,
            delta_status: git_delta.status(),
            delta_index: index,
        })
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
#[derive(Debug, Clone)]
pub struct DeltaFileDesc {
    path: Option<Utf8PathBuf>,
    _oid: git2::Oid,
    binary: bool,
}
impl Display for DeltaFileDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.path {
            Some(ref path) => {
                if self.binary {
                    f.write_str("binary ")?;
                }
                Display::fmt(path, f)
            }
            None => f.write_str("/dev/null"),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum BadPathError {
    #[error(transparent)]
    AbsolutePath(#[from] AbsolutePathError),
    #[error(transparent)]
    InvalidUtf8Path(#[from] camino::FromPathBufError),
}

impl BadPathError {
    fn set_role(&mut self, new_role: &'static str) {
        if let BadPathError::AbsolutePath(AbsolutePathError { ref mut role, .. }) = *self {
            *role = Some(new_role);
        }
    }
}
impl<'a> TryFrom<git2::DiffFile<'a>> for DeltaFileDesc {
    type Error = BadPathError;

    fn try_from(git_file: git2::DiffFile<'a>) -> Result<Self, Self::Error> {
        match git_file.path() {
            Some(path) if path.is_absolute() => {
                Err(BadPathError::AbsolutePath(AbsolutePathError {
                    role: None,
                    path: git_file.path().unwrap().to_path_buf().try_into()?,
                }))
            }
            None | Some(_) => Ok(DeltaFileDesc {
                path: git_file
                    .path()
                    .map(std::path::PathBuf::from)
                    .map(Utf8PathBuf::try_from)
                    .transpose()?,
                binary: git_file.is_binary(),
                _oid: git_file.id(),
            }),
        }
    }
}
