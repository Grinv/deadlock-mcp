//! The Deadlock Wiki (`MediaWiki` API). Its text is CC BY-NC-SA 4.0, so every answer carries the
//! page link and the license.

use std::time::Duration;

use schemars::JsonSchema;
use serde::Serialize;

use crate::http::{Fresh, Source, encode, get_json};
use crate::problem::{Kind, Problem, Result};

const API: &str = "https://deadlock.wiki/api.php";
const FRESH: Fresh = Fresh::For(Duration::from_hours(24));
const LICENSE: &str = "Text from the Deadlock Wiki, CC BY-NC-SA 4.0 (https://creativecommons.org/licenses/by-nc-sa/4.0/)";
/// Keeps a long page from filling the model's context.
const MAX_CHARS: usize = 12_000;

#[derive(Serialize, JsonSchema)]
pub struct SearchHit {
    pub title: String,
    pub snippet: String,
    pub url: String,
}

#[derive(Serialize, JsonSchema)]
pub struct Search {
    pub results: Vec<SearchHit>,
    pub license: &'static str,
    pub sources: Vec<Source>,
}

#[derive(Serialize, JsonSchema)]
pub struct Page {
    pub title: String,
    pub url: String,
    pub text: String,
    /// The page's sections, for asking for one of them.
    pub sections: Vec<String>,
    /// True when `text` was cut at 12,000 characters; ask for a section to read the rest.
    pub truncated: bool,
    pub license: &'static str,
    pub sources: Vec<Source>,
}

fn page_url(title: &str) -> String {
    format!("https://deadlock.wiki/{}", encode(&title.replace(' ', "_")))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&amp;", "&")
}

pub fn search(query: &str, limit: u32) -> Result<Search> {
    let url = format!(
        "{API}?action=query&list=search&format=json&srlimit={}&srsearch={}",
        limit.clamp(1, 20),
        encode(query)
    );
    let (data, source) = get_json(&url, FRESH)?;
    let results = data["query"]["search"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let title = r["title"].as_str()?;
            Some(SearchHit {
                title: title.to_owned(),
                snippet: strip_tags(r["snippet"].as_str().unwrap_or_default()),
                url: page_url(title),
            })
        })
        .collect();
    Ok(Search {
        results,
        license: LICENSE,
        sources: vec![source],
    })
}

pub fn page(title: &str, section: Option<&str>) -> Result<Page> {
    let url = format!(
        "{API}?action=query&prop=extracts&explaintext=1&redirects=1&format=json&titles={}",
        encode(title)
    );
    let (data, source) = get_json(&url, FRESH)?;
    let Some(page) = data["query"]["pages"]
        .as_object()
        .and_then(|p| p.values().next())
    else {
        return Err(
            Problem::new(Kind::InvalidResponse, "the wiki returned no page").with_instance(url),
        );
    };
    if page.get("missing").is_some() {
        return Err(Problem::not_found(format!(
            "the wiki has no page titled {title:?}; find the exact title with wiki_search"
        )));
    }
    let real_title = page["title"].as_str().unwrap_or(title).to_owned();
    let full = page["extract"].as_str().unwrap_or_default();
    let sections: Vec<String> = full
        .lines()
        .filter_map(heading)
        .map(|(_, name)| name)
        .collect();
    let text = match section {
        Some(s) => extract_section(full, s).ok_or_else(|| {
            Problem::invalid_argument(format!(
                "{real_title:?} has no section {s:?}; its sections are: {}",
                sections.join(", ")
            ))
        })?,
        None => full.to_owned(),
    };
    let truncated = text.chars().count() > MAX_CHARS;
    Ok(Page {
        url: page_url(&real_title),
        title: real_title,
        text: text.chars().take(MAX_CHARS).collect(),
        sections,
        truncated,
        license: LICENSE,
        sources: vec![source],
    })
}

/// A `== heading ==` line (any level) as `(level, name)`.
fn heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim();
    let level = t.chars().take_while(|&c| c == '=').count();
    (level >= 2 && t.ends_with('=')).then(|| (level, t.trim_matches('=').trim().to_owned()))
}

/// The text under a heading, up to the next heading of the same or a higher level.
fn extract_section(text: &str, wanted: &str) -> Option<String> {
    let mut lines = text.lines();
    let level = loop {
        let line = lines.next()?;
        if let Some((level, name)) = heading(line)
            && name.eq_ignore_ascii_case(wanted)
        {
            break level;
        }
    };
    let body: Vec<&str> = lines
        .take_while(|l| heading(l).is_none_or(|(lv, _)| lv > level))
        .collect();
    Some(body.join("\n").trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_stops_at_the_next_heading_of_its_level() {
        let text = "intro\n== A ==\none\n=== A1 ===\ntwo\n== B ==\nthree";
        assert_eq!(
            extract_section(text, "a").as_deref(),
            Some("one\n=== A1 ===\ntwo")
        );
        assert_eq!(extract_section(text, "B").as_deref(), Some("three"));
        assert_eq!(extract_section(text, "C"), None);
    }

    #[test]
    fn urls_and_snippets() {
        assert_eq!(
            page_url("Sinner's Sacrifice"),
            "https://deadlock.wiki/Sinner%27s_Sacrifice"
        );
        assert_eq!(
            strip_tags("<span class=\"searchmatch\">Rift</span> &amp; urn"),
            "Rift & urn"
        );
    }
}
