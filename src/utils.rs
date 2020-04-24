use std::str::Lines;
use std::iter::Peekable;

pub struct SimpleParser<'a> {
    lines: Peekable<Lines<'a>>,
    line_number: usize
}
impl<'a> SimpleParser<'a> {
    pub fn new(s: &'a str) -> Self {
        SimpleParser {
            lines: s.lines().peekable(),
            line_number: 1
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
        where P: FnMut(&str) -> bool, H: FnMut(&str) {
        while let Ok(line) = self.peek() {
            if matcher(line) {
                handler(self.pop().unwrap());
            } else {
                break
            }
        }
    }
    pub fn skip_while<P: FnMut(&str) -> bool>(&mut self, matcher: P) {
        self.take_while(matcher, |_| {});
    }
    pub fn skip_whitespace(&mut self) {
        self.skip_while(|line| line.chars().all(|c| c.is_whitespace()));
    }
    pub fn take_until<P, H>(&mut self, mut matcher: P, mut handler: H) -> Result<&'a str, UnexpectedEof>
        where P: FnMut(&str) -> bool, H: FnMut(&str) {
        loop {
            let line = self.pop()?;
            if matcher(line) {
                return Ok(line)
            } else {
                handler(line);
            }
        }
    }
}

#[derive(Debug)]
pub struct UnexpectedEof;