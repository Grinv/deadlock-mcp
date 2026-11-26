//! Human-readable times for tool output.

/// `m:ss` for a time into a match.
pub fn mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// `YYYY-MM-DD` (UTC) for a unix timestamp.
pub fn date(unix: i64) -> String {
    // Howard Hinnant's days-from-civil, inverted.
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(mmss(1216), "20:16");
        assert_eq!(mmss(59), "0:59");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_795_618_080), "2026-11-25");
        assert_eq!(date(951_782_400), "2000-02-29");
    }
}
