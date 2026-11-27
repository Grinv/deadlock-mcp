//! The Deadlock Wiki (`MediaWiki` API). Its text is CC BY-NC-SA 4.0, so every answer carries the
//! page link and the license.

use std::time::Duration;

use serde::Deserialize;

use crate::http::{Fresh, Source, encode, get_json};
use crate::problem::{Kind, Problem, Result};

const API: &str = "https://deadlock.wiki/api.php";
const FRESH: Fresh = Fresh::For(Duration::from_hours(24));

pub fn page_url(title: &str) -> String {
    format!("https://deadlock.wiki/{}", encode(&title.replace(' ', "_")))
}

/// The parts of `MediaWiki` `action=query` answers this module reads.
#[derive(Deserialize)]
struct Query<T> {
    query: T,
}

#[derive(Deserialize)]
struct SearchList {
    search: Vec<RawHit>,
}

#[derive(Deserialize)]
pub struct RawHit {
    pub title: String,
    #[serde(default)]
    pub snippet: String,
}

#[derive(Deserialize)]
struct Pages {
    pages: std::collections::HashMap<String, RawPage>,
}

#[derive(Deserialize)]
pub struct RawPage {
    pub title: String,
    #[serde(default)]
    pub extract: String,
    /// Present (as an empty string) when the page doesn't exist.
    pub missing: Option<serde_json::Value>,
}

/// Full-text search hits, at most `limit`, with the snippets' markup.
pub fn search(query: &str, limit: u32) -> Result<(Vec<RawHit>, Source)> {
    let url = format!(
        "{API}?action=query&list=search&format=json&srlimit={}&srsearch={}",
        limit.clamp(1, 20),
        encode(query)
    );
    let (data, source): (Query<SearchList>, Source) = get_json(&url, FRESH)?;
    Ok((data.query.search, source))
}

/// A page's text as plain text, redirects followed.
pub fn page(title: &str) -> Result<(RawPage, Source)> {
    let url = format!(
        "{API}?action=query&prop=extracts&explaintext=1&redirects=1&format=json&titles={}",
        encode(title)
    );
    let (data, source): (Query<Pages>, Source) = get_json(&url, FRESH)?;
    let Some(page) = data.query.pages.into_values().next() else {
        return Err(
            Problem::new(Kind::InvalidResponse, "the wiki returned no page").with_instance(url),
        );
    };
    Ok((page, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_snippets() {
        assert_eq!(
            page_url("Sinner's Sacrifice"),
            "https://deadlock.wiki/Sinner%27s_Sacrifice"
        );
    }
}
