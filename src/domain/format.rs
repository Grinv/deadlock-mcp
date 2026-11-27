//! Human-readable text for tool output: times, dates, and plain text from people and markup.

/// `m:ss` for a time into a match.
pub fn mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Text from people (Steam names, wiki pages) without control, zero-width and bidirectional
/// formatting characters, which can hide or reorder what the model reads. Newlines and tabs stay.
pub fn plain(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            let control = c.is_control() && !matches!(c, '\n' | '\t');
            let hidden = matches!(
                c,
                '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2060}'..='\u{2069}'
                    | '\u{FEFF}'
            );
            !(control || hidden)
        })
        .collect()
}

/// Plain text from HTML and the game's tooltip markup: tags dropped (block tags become line
/// breaks, list items `- ` lines), common entities decoded, runs of blanks collapsed. Line breaks
/// in the source count as spaces, as in HTML.
pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut tag: Option<String> = None;
    for c in s.chars() {
        match (tag.as_mut(), c) {
            (None, '<') => tag = Some(String::new()),
            // As in HTML, a line break in the source is a space; tags make the breaks.
            (None, '\n' | '\r') => out.push(' '),
            (None, c) => out.push(c),
            (Some(t), '>') => {
                let name: String = t
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '/')
                    .collect::<String>()
                    .to_ascii_lowercase();
                match name.as_str() {
                    "li" => out.push_str("\n- "),
                    "/li" => {}
                    n if matches!(
                        n.trim_start_matches('/'),
                        "br" | "p" | "div" | "ul" | "ol" | "tr" | "h1" | "h2" | "h3" | "h4"
                    ) =>
                    {
                        out.push('\n');
                    }
                    _ => {}
                }
                tag = None;
            }
            // Only the tag name matters; SVG paths can be long.
            (Some(t), c) if t.len() < 8 => t.push(c),
            (Some(_), _) => {}
        }
    }
    let out = out
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let mut lines: Vec<String> = Vec::new();
    for line in out.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !(line.is_empty() && lines.last().is_none_or(String::is_empty)) {
            lines.push(line);
        }
    }
    lines.join("\n").trim().to_owned()
}

/// At most `max` characters of `s`, and whether it was cut.
pub fn bounded(s: &str, max: usize) -> (String, bool) {
    (s.chars().take(max).collect(), s.chars().nth(max).is_some())
}

/// `s` cut at `max` characters, with `…` when something was cut.
pub fn clip(s: &str, max: usize) -> String {
    let (mut t, cut) = bounded(s, max);
    if cut {
        t.push('…');
    }
    t
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
    fn plain_drops_hidden_characters() {
        assert_eq!(plain("a\u{202E}b\u{200B}c\u{7}\nd"), "abc\nd");
    }

    #[test]
    fn strips_markup() {
        assert_eq!(
            strip_tags("<span class=\"searchmatch\">Rift</span> &amp; urn"),
            "Rift & urn"
        );
        assert_eq!(
            strip_tags(
                "<p>Fixes:</p><ul><li>One  <b>thing</b></li><li>Two</li></ul>end<br>with <svg>\n<path d=\"M1 2\"/>\n</svg> icon"
            ),
            "Fixes:\n\n- One thing\n- Two\nend\nwith icon"
        );
        assert_eq!(bounded("abc", 2), ("ab".to_owned(), true));
        assert_eq!(bounded("ab", 2), ("ab".to_owned(), false));
        assert_eq!(clip("a\nbc", 3), "a\nb…");
        assert_eq!(clip("ab", 2), "ab");
    }

    #[test]
    fn formats() {
        assert_eq!(mmss(1216), "20:16");
        assert_eq!(mmss(59), "0:59");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_795_618_080), "2026-11-25");
        assert_eq!(date(951_782_400), "2000-02-29");
    }
}
