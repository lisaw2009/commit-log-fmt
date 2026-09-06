# commit-log-fmt

A streaming, validating parser for git's raw commit log format, plus a
pretty printer that normalizes the output.

## The problem

`git log --format=raw` (and `git cat-file --batch` fed a list of commit
ids) prints commits in a plain text format that's easy for a human to
skim but tedious to work with programmatically: names and emails are
unquoted, timestamps are raw unix seconds plus a timezone offset, and
there's no schema enforcement anywhere in the pipeline. Tools that read
this output tend to slurp the whole thing into a `String` before parsing
it, which is fine for a small repo and then gets slow, or runs out of
memory, on something like the Linux kernel history: millions of commits.

This project parses the raw format one commit at a time from any
`BufRead`, so peak memory use is bounded by the size of a single commit's
metadata and message, not by the size of the whole history. It also
validates structure as it goes: sha shape, required fields, well-formed
author/committer lines, so malformed input is reported with a line
number instead of silently producing garbage.

## Usage

    git log --format=raw | commit-log-fmt

or against a saved log file:

    git log --format=raw > history.raw
    commit-log-fmt history.raw

Input commit:

    commit 3f7e8a2b1c9d0e4f5a6b7c8d9e0f1a2b3c4d5e6f
    tree 9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b
    parent 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b
    author Jane Doe <jane@example.com> 1700000000 -0500
    committer Jane Doe <jane@example.com> 1700000000 -0500

        Fix off-by-one in the tokenizer

        The loop condition used <= instead of <, which read one byte
        past the end of the buffer on the last token.

Output:

    commit 3f7e8a2b1c9d0e4f5a6b7c8d9e0f1a2b3c4d5e6f
    tree   9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b
    parent 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b
    Author:     Jane Doe <jane@example.com>
    AuthorDate: 2023-11-14T17:13:20-05:00
    Committer:  Jane Doe <jane@example.com>
    CommitDate: 2023-11-14T17:13:20-05:00

        Fix off-by-one in the tokenizer

        The loop condition used <= instead of <, which read one byte
        past the end of the buffer on the last token.

If a commit is malformed, parsing stops and the line number is reported
on stderr, e.g. `commit-log-fmt: line 42: author line missing '<'`.

## Design

- `src/parser.rs` - `CommitReader<R: BufRead>`, an iterator that pulls one
  commit's worth of lines at a time and validates it before yielding it.
- `src/model.rs` - the `Commit`/`Signature` types the parser produces.
- `src/printer.rs` - turns a `Commit` back into normalized text, including
  a from-scratch unix-timestamp-to-civil-date conversion (no date/time
  crate needed just to format a timestamp).

## Status

Early. Handles the common case of `git log --format=raw` without
`--decorate`, GPG signatures, or mergetags. See the roadmap in the issue
tracker for what's missing.
