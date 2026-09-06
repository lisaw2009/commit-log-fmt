#[derive(Debug, Clone)]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub timestamp: i64,
    /// Offset from UTC, in minutes, positive east of UTC (e.g. -0500 -> -300).
    pub tz_offset_minutes: i32,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub author: Signature,
    pub committer: Signature,
    /// Message lines with the raw format's 4-space indent already stripped.
    pub message: Vec<String>,
}
