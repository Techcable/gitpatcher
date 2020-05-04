use chrono::{DateTime, FixedOffset};
use git2::{Oid, Diff, Repository, ApplyLocation, Signature};
use regex::{Regex, Captures};
use lazy_static::lazy_static;
use std::fmt::{self, Display, Formatter};

pub struct EmailMessage {
     upstream_commit: Oid,
     date: DateTime<FixedOffset>,
     message_summary: String,
     message_tail: String,
     author_name: String,
     author_email: String,
     diff: Diff<'static>
}
lazy_static! {
     static ref HEADER_LINE: Regex = Regex::new("^From ([0-9A-Fa-f]{1,40}) Mon Sep 17 00:00:00 2001$").unwrap();
     static ref AUTHOR_LINE: Regex = Regex::new("^From: (.*) <(.*)>$").unwrap();
     static ref DATE_LINE: Regex = Regex::new(r#"^Date: (.* [\+-]\d+)$"#).unwrap();
     static ref SUBJECT_LINE: Regex = Regex::new(r#"^Subject: \[PATCH\] (.*)$"#).unwrap();
     static ref BEGIN_DIFF_LINE: Regex = Regex::new(r#"^diff --git a/(.*) b/(.*)$"#).unwrap();
}
fn match_header_line<'a>(
     lines: &mut dyn Iterator<Item=&'a str>,
     expected: &'static str,
     pattern: &Regex
) -> Result<Captures<'a>, InvalidEmailMessage> {
     let line = lines.next()
         .ok_or(InvalidEmailMessage::UnexpectedEof { expected })?;
     pattern.captures(line).ok_or_else(|| InvalidEmailMessage::InvalidHeader {
          expected, actual: line.into()
     })
}
impl EmailMessage {
     pub fn parse(msg: &str) -> Result<Self, InvalidEmailMessage> {
          let diff = Diff::from_buffer(msg.as_bytes())?;

          let mut lines = msg.lines().peekable();
          let header = match_header_line(
               &mut lines,
               "header",
               &*HEADER_LINE
          )?;
          let author = match_header_line(
               &mut lines,
               "author",
               &*AUTHOR_LINE
          )?;
          let date = match_header_line(
               &mut lines,
               "date",
               &*DATE_LINE
          )?;
          let subject = match_header_line(
               &mut lines,
               "subject",
               &*SUBJECT_LINE
          )?;
          let mut message_subject = String::from(&subject[1]);
          loop {
               let line = lines.next().ok_or(InvalidEmailMessage::UnexpectedEof {
                    expected: "diff after subject"
               })?;
               if line.is_empty() {
                    break
               } else {
                    // Breaking over newlines doesn't affect final result
                    message_subject.push_str(line);
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
                    expected: "diff after message"
               })?;
               if line.is_empty() {
                    match lines.peek() {
                         Some(line) if BEGIN_DIFF_LINE.is_match(line) => break,
                         _ => {
                              trailing_message.push('\n');
                              // NOTE: None is implicitly handled by error in next iteration
                              continue
                         },

                    }
               } else {
                    trailing_message.push_str(line);
                    trailing_message.push('\n');
               }
          }
          if trailing_message.ends_with('\n') {
               assert_eq!(trailing_message.pop(), Some('\n'));
          }
          let author_name = &author[1];
          let author_email = &author[2];
          let date = DateTime::parse_from_rfc2822(&date[1])
              .map_err(|cause| {
                   InvalidEmailMessage::InvalidDate { cause, actual: date[1].into() }
              })?;
          let upstream_commit = Oid::from_str(&header[1]).unwrap();
          Ok(EmailMessage {
               diff, upstream_commit, date,
               message_summary: message_subject,
               message_tail: trailing_message,
               author_name: author_name.into(),
               author_email: author_email.into()
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
          target.apply(&self.diff, ApplyLocation::Both, None)?;
          let time = git2::Time::new(
               self.date.timestamp(),
               // seconds -> minutes
               self.date.timezone().local_minus_utc() / 60
          );
          let author = Signature::new(
               &self.author_name,
               &self.author_email,
               &time
          )?;
          let tree = target.index()?.write_tree_to(target)?;
          let tree = target.find_tree(tree)?;
          // TODO: Handle detatched head/no commits
          let head_commit = target.head()?.peel_to_commit()?;
          let parents = vec![&head_commit];
          let message = self.full_message();
          target.commit(
               Some("HEAD"),
               &author,
               &author,
               &message,
               &tree,
               &parents
          )?;
          Ok(())
     }
}

#[derive(Debug)]
pub enum InvalidEmailMessage {
     UnexpectedEof {
          expected: &'static str
     },
     InvalidHeader {
          expected: &'static str,
          actual: String
     },
     InvalidDate {
          actual: String,
          cause: chrono::ParseError
     },
     Git(git2::Error)
}
impl From<git2::Error> for InvalidEmailMessage {
     fn from(cause: git2::Error) -> Self {
          InvalidEmailMessage::Git(cause)
     }
}

impl Display for InvalidEmailMessage {
     fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
          match self {
               InvalidEmailMessage::UnexpectedEof { expected } => {
                    write!(f, "Unexpected EOF, expected {}", expected)
               },
               InvalidEmailMessage::InvalidHeader { expected, actual } => {
                    write!(f, "Invalid header line, expeted {}: {:?}", expected, actual)
               },
               InvalidEmailMessage::InvalidDate { actual, cause } => {
                    write!(f, "Invalid date {:?}: {}", actual, cause)
               },
               InvalidEmailMessage::Git(cause) => {
                    write!(f, "Internal git error: {}", cause)
               },
          }
     }
}