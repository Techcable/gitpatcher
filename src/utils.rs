use std::iter::Peekable;

use arrayvec::ArrayVec;
use bstr::{BStr, ByteSlice};

pub struct SimpleParser<'a> {
    lines: Peekable<bstr::Lines<'a>>,
    line_number: usize,
}
impl<'a> SimpleParser<'a> {
    pub fn new(s: &'a BStr) -> Self {
        SimpleParser {
            lines: s.lines().peekable(),
            line_number: 1,
        }
    }
    #[inline]
    pub fn peek(&mut self) -> Result<&'a BStr, UnexpectedEof> {
        match self.lines.peek() {
            Some(&line) => Ok(BStr::new(line)),
            None => Err(UnexpectedEof),
        }
    }
    #[inline]
    pub fn pop(&mut self) -> Result<&'a BStr, UnexpectedEof> {
        let line = self.lines.next().ok_or(UnexpectedEof)?;
        self.line_number += 1;
        Ok(BStr::new(line))
    }
    pub fn take_while(
        &mut self,
        mut matcher: impl FnMut(&BStr) -> bool,
        mut handler: impl FnMut(&BStr),
    ) {
        while let Ok(line) = self.peek() {
            if matcher(line) {
                handler(self.pop().unwrap());
            } else {
                break;
            }
        }
    }
    pub fn skip_while<P: FnMut(&BStr) -> bool>(&mut self, matcher: P) {
        self.take_while(matcher, |_| {});
    }
    pub fn skip_whitespace(&mut self) {
        self.skip_while(|line| line.chars().all(|c| c.is_whitespace()));
    }
    pub fn take_until(
        &mut self,
        mut matcher: impl FnMut(&BStr) -> bool,
        mut handler: impl FnMut(&BStr),
    ) -> Result<&'a BStr, UnexpectedEof> {
        loop {
            let line = self.pop()?;
            if matcher(line) {
                return Ok(line);
            } else {
                handler(line);
            }
        }
    }
}

#[derive(Debug)]
pub struct RememberLast<T: Clone, const LIMIT: usize> {
    // Items we remember from oldest to newest
    last: ArrayVec<T, LIMIT>,
}
impl<T: Clone, const LIMIT: usize> RememberLast<T, LIMIT> {
    pub fn new() -> Self {
        assert!(LIMIT > 0);
        RememberLast {
            last: ArrayVec::new(),
        }
    }
    pub fn remember(&mut self, element: &T) {
        if self.last.len() < LIMIT {
            self.last.push(element.clone());
        } else {
            self.last.rotate_left(1);
            Clone::clone_from(self.last.last_mut().unwrap(), element);
        }
    }
    #[inline]
    pub fn back(&self, offset: usize) -> &T {
        &self.last[self.last.len() - offset - 1]
    }
    /// The last elements we remember,
    /// from oldest to newest
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.last
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.last.len()
    }
}
pub struct IterRememberLast<I: Iterator, const LIMIT: usize>
where
    I::Item: Clone,
{
    remember: RememberLast<I::Item, LIMIT>,
    iter: I,
}
impl<I: Iterator, const LIMIT: usize> IterRememberLast<I, LIMIT> where I::Item: Clone {}
impl<I: Iterator, const LIMIT: usize> Iterator for IterRememberLast<I, LIMIT>
where
    I::Item: Clone,
{
    type Item = I::Item;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(element) = self.iter.next() {
            self.remember.remember(&element);
            Some(element)
        } else {
            None
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unexpected EOF")]
pub struct UnexpectedEof;

/// Utilities for logging
pub mod log {
    use std::path::{Path, PathBuf};

    /// Wrapper for [PathBuf] that implements [slog::Value]
    #[derive(Debug, Clone)]
    pub struct LogPathValue(pub PathBuf);
    impl From<PathBuf> for LogPathValue {
        #[inline]
        fn from(value: PathBuf) -> Self {
            LogPathValue(value)
        }
    }

    impl<'a> From<&'a Path> for LogPathValue {
        #[inline]
        fn from(value: &'a Path) -> Self {
            LogPathValue(value.to_path_buf())
        }
    }

    impl slog::Value for LogPathValue {
        fn serialize(
            &self,
            record: &slog::Record,
            key: slog::Key,
            serializer: &mut dyn slog::Serializer,
        ) -> slog::Result {
            slog::Value::serialize(&self.0.display(), record, key, serializer)
        }
    }
}
