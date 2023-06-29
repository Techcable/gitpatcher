use git2::{ApplyLocation, Diff, Repository, Signature};
use nom::bytes::complete::{tag, take_until, take_until1, take_while1, take_while_m_n};
use nom::character::{is_digit, is_hex_digit};
use nom::combinator::{all_consuming, opt, recognize, rest};
use nom::{sequence::tuple, IResult};
use time::format_description::well_known::Rfc2822;
use time::OffsetDateTime;

pub struct EmailMessage<'a> {
    date: OffsetDateTime,
    message_summary: String,
    message_tail: String,
    author_name: &'a str,
    author_email: &'a str,
    git_diff: Diff<'static>,
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
        IResult::Ok((remaining, value)) => {
            assert_eq!(remaining, b"");
            Ok(value)
        }
        IResult::Err(nom::Err::Error(err)) | IResult::Err(nom::Err::Failure(err)) => {
            Err(InvalidEmailMessage::InvalidHeader {
                actual: line.into(),
                expected,
                reason: nom::error::Error {
                    input: std::str::from_utf8(err.input)?.into(),
                    code: err.code,
                },
            })
        }
        IResult::Err(nom::Err::Incomplete(_)) => unreachable!(),
    }
}
impl<'a> EmailMessage<'a> {
    // TODO: Accept bstr?
    pub fn parse(msg: &'a str) -> Result<Self, InvalidEmailMessage> {
        let git_diff = Diff::from_buffer(msg.as_bytes())?;

        let mut lines = msg.lines().peekable();
        match_header_line(&mut lines, "header", parse_header_line)?;
        let author = match_header_line(&mut lines, "author", parse_author_line)?
            .try_map(std::str::from_utf8)?;
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
                //
                // TODO: Avoid copying into memory?
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
        let author_name = &author.name;
        let author_email = &author.email;
        let date = OffsetDateTime::parse(&date, &Rfc2822).map_err(|cause| {
            InvalidEmailMessage::InvalidDate {
                cause,
                actual: date.into(),
            }
        })?;
        Ok(EmailMessage {
            git_diff,
            date,
            message_summary: message_summary,
            message_tail: trailing_message,
            author_name: author_name,
            author_email: author_email,
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
    pub fn apply_commit(&self, target: &Repository) -> Result<(), git2::Error> {
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

pub use self::owned::OwnedEmailMessage;

/// Helper module for using owned references to [EmailMessage] (which is otherwise borrowed).
///
/// Unfortunately, this involves unsafe code, which is why it is in a seperate module.
mod owned {
    use super::EmailMessage;
    use stable_deref_trait::StableDeref;

    /// Wrapper around a [`EmailMessage`] that owns a reference to the string.
    ///
    /// Needed because Rust has no self-referential structs...
    pub struct OwnedEmailMessage<T: StableDeref<Target = str> = String> {
        text: T,
        msg: EmailMessage<'static>,
    }
    impl<T: StableDeref<Target = str>> OwnedEmailMessage<T> {
        pub fn try_init<E>(
            text: T,
            func: impl for<'a> FnOnce(&'a T) -> Result<EmailMessage<'a>, E>,
        ) -> Result<Self, E> {
            let msg = func(&text)?;
            // Erase lifetime
            let msg =
                unsafe { std::mem::transmute::<EmailMessage<'_>, EmailMessage<'static>>(msg) };
            Ok(OwnedEmailMessage { text, msg })
        }
    }
    impl<T: StableDeref<Target = str>> OwnedEmailMessage<T> {
        #[inline]
        pub fn text(&self) -> &str {
            &self.text
        }
        #[inline]
        pub fn email<'a>(&'a self) -> &'a EmailMessage<'a> {
            unsafe {
                let this: &'a OwnedEmailMessage<T> = self;
                &*(&this.msg as &'a EmailMessage<'static> as *const EmailMessage<'static>
                    as *const EmailMessage<'a>)
            }
        }
    }
}
