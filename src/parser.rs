use std::io::{self, BufRead};
use std::iter::Peekable;

use crate::model::{Commit, Signature};

#[derive(Debug)]
pub enum ParseError {
    Io(io::Error),
    Malformed { line: usize, reason: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "io error: {}", e),
            ParseError::Malformed { line, reason } => write!(f, "line {}: {}", line, reason),
        }
    }
}

impl std::error::Error for ParseError {}

fn is_hex_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_commit_header(line: &str) -> bool {
    line.strip_prefix("commit ").map_or(false, is_hex_sha)
}

fn parse_tz_offset(tz: &str) -> Option<i32> {
    if tz.len() != 5 {
        return None;
    }
    let sign = match tz.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = &tz[1..];
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hh: i32 = digits[0..2].parse().ok()?;
    let mm: i32 = digits[2..4].parse().ok()?;
    if mm >= 60 {
        return None;
    }
    Some(sign * (hh * 60 + mm))
}

/// Reads commits one at a time from git's raw log format
/// (`git log --format=raw`). Only a single commit's lines are ever held in
/// memory at once, so this scales to histories far larger than available
/// RAM as long as the caller doesn't collect every yielded `Commit`.
pub struct CommitReader<R: BufRead> {
    lines: Peekable<io::Lines<R>>,
    line_no: usize,
}

impl<R: BufRead> CommitReader<R> {
    pub fn new(reader: R) -> Self {
        CommitReader {
            lines: reader.lines().peekable(),
            line_no: 0,
        }
    }

    fn next_line(&mut self) -> Option<Result<String, ParseError>> {
        let item = self.lines.next()?;
        self.line_no += 1;
        Some(item.map_err(ParseError::Io))
    }

    fn take_line(&mut self) -> Result<String, ParseError> {
        match self.next_line() {
            Some(r) => r,
            None => Err(self.fail("unexpected end of input")),
        }
    }

    /// Clones the next line without consuming it. Cloning keeps the borrow
    /// checker out of the way; it costs one short-lived allocation per line,
    /// not per commit, so it doesn't change the overall memory profile.
    fn peek_str(&mut self) -> Result<Option<String>, ParseError> {
        match self.lines.peek() {
            None => Ok(None),
            Some(Ok(s)) => Ok(Some(s.clone())),
            Some(Err(_)) => Err(self.take_line().unwrap_err()),
        }
    }

    fn fail(&self, reason: impl Into<String>) -> ParseError {
        ParseError::Malformed {
            line: self.line_no,
            reason: reason.into(),
        }
    }

    fn expect_prefixed(&mut self, prefix: &str) -> Result<String, ParseError> {
        let line = self.take_line()?;
        match line.strip_prefix(prefix) {
            Some(rest) => Ok(rest.to_string()),
            None => Err(self.fail(format!(
                "expected a line starting with {:?}, found {:?}",
                prefix, line
            ))),
        }
    }

    fn parse_signature(&self, field: &str, line: &str) -> Result<Signature, ParseError> {
        let lt = line
            .find('<')
            .ok_or_else(|| self.fail(format!("{} line missing '<'", field)))?;
        let gt = line
            .find('>')
            .ok_or_else(|| self.fail(format!("{} line missing '>'", field)))?;
        if gt < lt {
            return Err(self.fail(format!("{} line has '>' before '<'", field)));
        }
        let name = line[..lt].trim().to_string();
        if name.is_empty() {
            return Err(self.fail(format!("{} line is missing a name", field)));
        }
        let email = line[lt + 1..gt].to_string();
        let rest = line[gt + 1..].trim();
        let mut parts = rest.split_whitespace();
        let ts_str = parts
            .next()
            .ok_or_else(|| self.fail(format!("{} line is missing a timestamp", field)))?;
        let tz_str = parts
            .next()
            .ok_or_else(|| self.fail(format!("{} line is missing a timezone", field)))?;
        if parts.next().is_some() {
            return Err(self.fail(format!("{} line has unexpected trailing data", field)));
        }
        let timestamp: i64 = ts_str.parse().map_err(|_| {
            self.fail(format!("{} line has an invalid timestamp {:?}", field, ts_str))
        })?;
        let tz_offset_minutes = parse_tz_offset(tz_str).ok_or_else(|| {
            self.fail(format!("{} line has an invalid timezone {:?}", field, tz_str))
        })?;
        Ok(Signature {
            name,
            email,
            timestamp,
            tz_offset_minutes,
        })
    }

    fn parse_one(&mut self) -> Result<Commit, ParseError> {
        let sha = self.expect_prefixed("commit ")?;
        if !is_hex_sha(&sha) {
            return Err(self.fail(format!("commit id {:?} is not a 40-character hex sha", sha)));
        }
        let tree = self.expect_prefixed("tree ")?;
        if !is_hex_sha(&tree) {
            return Err(self.fail(format!("tree id {:?} is not a 40-character hex sha", tree)));
        }

        let mut parents = Vec::new();
        while matches!(self.peek_str()?, Some(ref l) if l.starts_with("parent ")) {
            let parent = self.expect_prefixed("parent ")?;
            if !is_hex_sha(&parent) {
                return Err(self.fail(format!(
                    "parent id {:?} is not a 40-character hex sha",
                    parent
                )));
            }
            parents.push(parent);
        }

        let author_line = self.expect_prefixed("author ")?;
        let author = self.parse_signature("author", &author_line)?;
        let committer_line = self.expect_prefixed("committer ")?;
        let committer = self.parse_signature("committer", &committer_line)?;

        let separator = self.take_line()?;
        if !separator.is_empty() {
            return Err(self.fail(format!(
                "expected a blank line before the message, found {:?}",
                separator
            )));
        }

        // Message lines carry a 4-space indent. A blank line only ends the
        // message if what follows is the next commit header (or nothing) -
        // otherwise it's a blank paragraph break inside the message itself.
        let mut message = Vec::new();
        loop {
            match self.peek_str()? {
                None => break,
                Some(ref l) if l.is_empty() => {
                    self.take_line()?;
                    match self.peek_str()? {
                        Some(ref next) if is_commit_header(next) => break,
                        None => break,
                        _ => message.push(String::new()),
                    }
                }
                Some(ref l) if l.starts_with("    ") => {
                    let line = self.take_line()?;
                    message.push(line[4..].to_string());
                }
                Some(_) => break,
            }
        }

        Ok(Commit {
            sha,
            tree,
            parents,
            author,
            committer,
            message,
        })
    }
}

impl<R: BufRead> Iterator for CommitReader<R> {
    type Item = Result<Commit, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.peek_str() {
                Ok(Some(ref l)) if l.is_empty() => {
                    let _ = self.take_line();
                    continue;
                }
                Ok(None) => return None,
                Ok(Some(_)) => break,
                Err(e) => return Some(Err(e)),
            }
        }
        Some(self.parse_one())
    }
}
