use crate::model::Commit;

pub fn pretty_print(commit: &Commit) -> String {
    let mut out = String::new();
    out.push_str(&format!("commit {}\n", commit.sha));
    out.push_str(&format!("tree   {}\n", commit.tree));
    for parent in &commit.parents {
        out.push_str(&format!("parent {}\n", parent));
    }
    out.push_str(&format!(
        "{:<12}{} <{}>\n",
        "Author:", commit.author.name, commit.author.email
    ));
    out.push_str(&format!(
        "{:<12}{}\n",
        "AuthorDate:",
        format_timestamp(commit.author.timestamp, commit.author.tz_offset_minutes)
    ));
    out.push_str(&format!(
        "{:<12}{} <{}>\n",
        "Committer:", commit.committer.name, commit.committer.email
    ));
    out.push_str(&format!(
        "{:<12}{}\n",
        "CommitDate:",
        format_timestamp(commit.committer.timestamp, commit.committer.tz_offset_minutes)
    ));
    out.push('\n');
    for line in &commit.message {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Converts a unix timestamp plus a timezone offset (minutes east of UTC)
/// into an ISO-8601 string in that timezone. Implemented by hand (Howard
/// Hinnant's civil_from_days algorithm) so formatting a commit date doesn't
/// require pulling in a date/time crate.
fn format_timestamp(unix_ts: i64, tz_offset_minutes: i32) -> String {
    let local_ts = unix_ts + tz_offset_minutes as i64 * 60;
    let days = local_ts.div_euclid(86_400);
    let secs_of_day = local_ts.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let sign = if tz_offset_minutes < 0 { '-' } else { '+' };
    let offset_abs = tz_offset_minutes.unsigned_abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
        year,
        month,
        day,
        hour,
        minute,
        second,
        sign,
        offset_abs / 60,
        offset_abs % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}
