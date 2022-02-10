use std::iter::Peekable;
use std::str::Lines;

pub struct SimpleParser<'a> {
    lines: Peekable<Lines<'a>>,
    line_number: usize,
}
impl<'a> SimpleParser<'a> {
    pub fn new(s: &'a str) -> Self {
        SimpleParser {
            lines: s.lines().peekable(),
            line_number: 1,
        }
    }
    #[inline]
    pub fn peek(&mut self) -> Result<&'a str, UnexpectedEof> {
        self.lines.peek().cloned().ok_or(UnexpectedEof)
    }
    #[inline]
    pub fn pop(&mut self) -> Result<&'a str, UnexpectedEof> {
        let line = self.lines.next().ok_or(UnexpectedEof)?;
        self.line_number += 1;
        Ok(line)
    }
    pub fn take_while<P, H>(&mut self, mut matcher: P, mut handler: H)
    where
        P: FnMut(&str) -> bool,
        H: FnMut(&str),
    {
        while let Ok(line) = self.peek() {
            if matcher(line) {
                handler(self.pop().unwrap());
            } else {
                break;
            }
        }
    }
    pub fn skip_while<P: FnMut(&str) -> bool>(&mut self, matcher: P) {
        self.take_while(matcher, |_| {});
    }
    pub fn skip_whitespace(&mut self) {
        self.skip_while(|line| line.chars().all(|c| c.is_whitespace()));
    }
    pub fn take_until<P, H>(
        &mut self,
        mut matcher: P,
        mut handler: H,
    ) -> Result<&'a str, UnexpectedEof>
    where
        P: FnMut(&str) -> bool,
        H: FnMut(&str),
    {
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
pub struct RememberLast<T: Clone> {
    limit: usize,
    // Items we remember from oldest to newest
    last: Vec<T>,
}
impl<T: Clone> RememberLast<T> {
    pub fn new(limit: usize) -> Self {
        assert!(limit > 0);
        RememberLast {
            limit,
            last: Vec::with_capacity(limit),
        }
    }
    pub fn remember(&mut self, element: &T) {
        if self.last.len() < self.limit {
            self.last.push(element.clone());
        } else {
            self.last.rotate_left(1);
            Clone::clone_from(self.last.last_mut().unwrap(), &element);
        }
    }
    #[inline]
    pub fn back(&self, offset: usize) -> &T {
        &self.last[self.last.len() - offset - 1]
    }
    /// The last elements we remember,
    /// from oldest to newes
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.last
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.last.len()
    }
}
pub struct IterRememberLast<I: Iterator>
where
    I::Item: Clone,
{
    remember: RememberLast<I::Item>,
    iter: I,
}
impl<I: Iterator> IterRememberLast<I> where I::Item: Clone {}
impl<I: Iterator> Iterator for IterRememberLast<I>
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

#[derive(Debug)]
pub struct UnexpectedEof;
