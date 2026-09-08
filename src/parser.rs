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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SHA1: &str = "3f7e8a2b1c9d0e4f5a6b7c8d9e0f1a2b3c4d5e6f";
    const TREE1: &str = "9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b";
    const PARENT1: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";

    fn parse_all(input: &str) -> Vec<Result<Commit, ParseError>> {
        CommitReader::new(Cursor::new(input.as_bytes())).collect()
    }

    fn one_error(input: &str) -> ParseError {
        let mut results = parse_all(input);
        assert_eq!(results.len(), 1, "expected exactly one item from {:?}", input);
        results.pop().unwrap().unwrap_err()
    }

    fn err_line(e: &ParseError) -> usize {
        match e {
            ParseError::Malformed { line, .. } => *line,
            ParseError::Io(_) => panic!("expected a Malformed error, got Io"),
        }
    }

    fn basic_commit(extra_parents: &str, message: &str) -> String {
        format!(
            "commit {sha}\ntree {tree}\n{parents}author Jane Doe <jane@example.com> 1700000000 -0500\ncommitter Jane Doe <jane@example.com> 1700000000 -0500\n\n{message}",
            sha = SHA1,
            tree = TREE1,
            parents = extra_parents,
            message = message,
        )
    }

    #[test]
    fn parses_a_well_formed_commit() {
        let input = basic_commit("", "    Fix off-by-one in the tokenizer\n");
        let results = parse_all(&input);
        assert_eq!(results.len(), 1);
        let commit = results[0].as_ref().unwrap();
        assert_eq!(commit.sha, SHA1);
        assert_eq!(commit.tree, TREE1);
        assert!(commit.parents.is_empty());
        assert_eq!(commit.author.name, "Jane Doe");
        assert_eq!(commit.author.email, "jane@example.com");
        assert_eq!(commit.author.timestamp, 1_700_000_000);
        assert_eq!(commit.author.tz_offset_minutes, -300);
        assert_eq!(commit.message, vec!["Fix off-by-one in the tokenizer".to_string()]);
    }

    #[test]
    fn parses_a_merge_commit_with_multiple_parents() {
        let parents = format!("parent {p}\nparent {p}\n", p = PARENT1);
        let input = basic_commit(&parents, "    Merge branch 'foo'\n");
        let results = parse_all(&input);
        assert_eq!(results.len(), 1);
        let commit = results[0].as_ref().unwrap();
        assert_eq!(commit.parents, vec![PARENT1.to_string(), PARENT1.to_string()]);
    }

    #[test]
    fn preserves_blank_paragraph_breaks_inside_the_message() {
        let input = basic_commit("", "    First paragraph.\n\n    Second paragraph.\n");
        let results = parse_all(&input);
        let commit = results[0].as_ref().unwrap();
        assert_eq!(
            commit.message,
            vec![
                "First paragraph.".to_string(),
                "".to_string(),
                "Second paragraph.".to_string(),
            ]
        );
    }

    #[test]
    fn parses_multiple_commits_separated_by_a_blank_line() {
        let one = basic_commit("", "    First\n");
        let two = basic_commit("", "    Second\n");
        let input = format!("{}\n{}", one, two);
        let results = parse_all(&input);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap().message, vec!["First".to_string()]);
        assert_eq!(results[1].as_ref().unwrap().message, vec!["Second".to_string()]);
    }

    #[test]
    fn parses_multiple_commits_with_no_separator() {
        let one = basic_commit("", "    First\n");
        let two = basic_commit("", "    Second\n");
        let input = format!("{}{}", one, two);
        let results = parse_all(&input);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap().message, vec!["First".to_string()]);
        assert_eq!(results[1].as_ref().unwrap().message, vec!["Second".to_string()]);
    }

    #[test]
    fn skips_leading_blank_lines_before_the_first_commit() {
        let input = format!("\n\n{}", basic_commit("", "    Hi\n"));
        let results = parse_all(&input);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
    }

    #[test]
    fn empty_input_yields_no_commits() {
        assert!(parse_all("").is_empty());
    }

    #[test]
    fn rejects_a_malformed_commit_sha() {
        let e = one_error("commit not-a-sha\n");
        assert!(e.to_string().contains("not a 40-character hex sha"));
        assert_eq!(err_line(&e), 1);
    }

    #[test]
    fn rejects_a_missing_tree_line() {
        let input = format!("commit {}\nauthor x <x@x> 1 +0000\n", SHA1);
        let e = one_error(&input);
        assert!(e.to_string().contains("expected a line starting with \"tree \""));
    }

    #[test]
    fn rejects_a_malformed_parent_sha() {
        let input = format!("commit {}\ntree {}\nparent nope\n", SHA1, TREE1);
        let e = one_error(&input);
        assert!(e.to_string().contains("parent id"));
        assert!(e.to_string().contains("not a 40-character hex sha"));
    }

    #[test]
    fn rejects_an_author_line_missing_open_angle_bracket() {
        let input = basic_commit("", "    x\n")
            .replace("author Jane Doe <jane@example.com>", "author Jane Doe jane@example.com>");
        let e = one_error(&input);
        assert!(e.to_string().contains("author line missing '<'"));
    }

    #[test]
    fn rejects_an_author_line_missing_close_angle_bracket() {
        let input = basic_commit("", "    x\n")
            .replace("author Jane Doe <jane@example.com>", "author Jane Doe <jane@example.com");
        let e = one_error(&input);
        assert!(e.to_string().contains("author line missing '>'"));
    }

    #[test]
    fn rejects_an_author_line_with_brackets_out_of_order() {
        let input = format!(
            "commit {}\ntree {}\nauthor > < 1700000000 -0500\ncommitter x <x@x> 1 +0000\n\n    x\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("has '>' before '<'"));
    }

    #[test]
    fn rejects_an_author_line_missing_a_name() {
        let input = format!(
            "commit {}\ntree {}\nauthor <jane@example.com> 1700000000 -0500\ncommitter x <x@x> 1 +0000\n\n    x\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("is missing a name"));
    }

    #[test]
    fn rejects_an_author_line_missing_a_timestamp() {
        let input = format!(
            "commit {}\ntree {}\nauthor Jane Doe <jane@example.com>\ncommitter x <x@x> 1 +0000\n\n    x\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("is missing a timestamp"));
    }

    #[test]
    fn rejects_an_author_line_missing_a_timezone() {
        let input = format!(
            "commit {}\ntree {}\nauthor Jane Doe <jane@example.com> 1700000000\ncommitter x <x@x> 1 +0000\n\n    x\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("is missing a timezone"));
    }

    #[test]
    fn rejects_an_author_line_with_trailing_data() {
        let input = format!(
            "commit {}\ntree {}\nauthor Jane Doe <jane@example.com> 1700000000 -0500 extra\ncommitter x <x@x> 1 +0000\n\n    x\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("unexpected trailing data"));
    }

    #[test]
    fn rejects_an_invalid_timestamp() {
        let input = format!(
            "commit {}\ntree {}\nauthor Jane Doe <jane@example.com> not-a-number -0500\ncommitter x <x@x> 1 +0000\n\n    x\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("invalid timestamp"));
    }

    #[test]
    fn rejects_an_invalid_timezone() {
        let input = format!(
            "commit {}\ntree {}\nauthor Jane Doe <jane@example.com> 1700000000 nope\ncommitter x <x@x> 1 +0000\n\n    x\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("invalid timezone"));
    }

    #[test]
    fn rejects_a_missing_blank_separator_before_the_message() {
        let input = format!(
            "commit {}\ntree {}\nauthor a <a@a> 1 +0000\ncommitter a <a@a> 1 +0000\n    not preceded by a blank line\n",
            SHA1, TREE1
        );
        let e = one_error(&input);
        assert!(e.to_string().contains("expected a blank line before the message"));
    }

    #[test]
    fn rejects_truncated_input() {
        let input = format!("commit {}\n", SHA1);
        let e = one_error(&input);
        assert!(e.to_string().contains("unexpected end of input"));
    }

    #[test]
    fn parse_tz_offset_handles_valid_and_invalid_input() {
        assert_eq!(parse_tz_offset("+0000"), Some(0));
        assert_eq!(parse_tz_offset("-0500"), Some(-300));
        assert_eq!(parse_tz_offset("+0530"), Some(330));
        assert_eq!(parse_tz_offset("+000"), None);
        assert_eq!(parse_tz_offset("+00500"), None);
        assert_eq!(parse_tz_offset("+0060"), None);
        assert_eq!(parse_tz_offset("+00a0"), None);
        assert_eq!(parse_tz_offset("00000"), None);
    }

    #[test]
    fn is_hex_sha_rejects_wrong_length_and_non_hex() {
        assert!(is_hex_sha(SHA1));
        assert!(!is_hex_sha(&SHA1[..39]));
        assert!(!is_hex_sha(&format!("{}g", &SHA1[..39])));
    }
}
