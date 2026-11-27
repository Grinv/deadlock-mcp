//! Tool logic and answer types; `src/server/` only routes MCP calls here.

pub mod game;
pub mod matches;
pub mod meta;
pub mod player;

use std::fmt::Write as _;

pub use crate::domain::id32;

use crate::domain::assets::Catalog;
use crate::http::Source;
use crate::problem::Result;

/// Time and rank filters of the stats tools, as the API's query parameters.
#[derive(Default)]
pub struct Filters {
    /// Lowest average rank badge of the match.
    pub min_badge: Option<u8>,
    pub max_badge: Option<u8>,
    /// Only the last this many days.
    pub days: Option<u32>,
}

impl Filters {
    /// `&key=value` pairs. The start time is rounded down to the hour, so repeated calls hit the
    /// local cache and the API's own one-hour cache.
    pub fn query(&self) -> String {
        self.query_at(crate::http::unix_now())
    }

    fn query_at(&self, now: u64) -> String {
        let mut q = String::new();
        if let Some(b) = self.min_badge {
            let _ = write!(q, "&min_average_badge={b}");
        }
        if let Some(b) = self.max_badge {
            let _ = write!(q, "&max_average_badge={b}");
        }
        if let Some(days) = self.days {
            let since = (now / 3600).saturating_sub(u64::from(days) * 24) * 3600;
            let _ = write!(q, "&min_unix_timestamp={since}");
        }
        q
    }
}

/// Wins over matches, 0..1 rounded to 3 places; `None` without matches.
#[expect(
    clippy::cast_precision_loss,
    reason = "match counts are far below 2^52"
)]
pub fn win_rate(wins: i64, matches: i64) -> Option<f64> {
    (matches > 0).then(|| round(wins as f64 / matches as f64, 3))
}

/// `x` rounded to `places` decimals, to keep answers short.
pub fn round(x: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (x * f).round() / f
}

/// `&hero_ids=<id>` for an optional hero, and the hero's name for the answer.
pub fn hero_filter(cat: &Catalog, hero: Option<&str>) -> Result<(String, Option<String>)> {
    let id = hero.map(|h| cat.find_hero(h)).transpose()?;
    Ok((
        id.map_or_default(|h| format!("&hero_ids={h}")),
        id.map(|h| cat.hero(h)),
    ))
}

/// The sources of an answer built with the catalog: its own, in order, then the catalog's degraded
/// ones.
pub fn answer_sources(own: impl IntoIterator<Item = Source>, cat: &Catalog) -> Vec<Source> {
    own.into_iter().chain(cat.degraded_sources()).collect()
}

/// `total / matches` rounded to 2 places; 0 without matches.
#[expect(clippy::cast_precision_loss, reason = "counts are far below 2^52")]
pub fn per(total: i64, matches: i64) -> f64 {
    if matches == 0 {
        0.0
    } else {
        round(total as f64 / matches as f64, 2)
    }
}

/// `count` as a `usize` for `take`.
pub fn take(count: u32) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer lists its own sources first, in order, then only the catalog's stale or failed
    /// ones.
    #[test]
    fn answer_sources_add_the_degraded_catalog_parts() {
        let source = |url: &str, stale| Source {
            url: url.to_owned(),
            fetched_at: 1,
            from_cache: true,
            stale,
            error: None,
        };
        let failed = Source::failed(
            &crate::problem::Problem::new(crate::problem::Kind::Unreachable, "down")
                .with_instance("map"),
        );
        let cat = Catalog::test(&[]).with_sources(vec![
            source("heroes", false),
            source("items", true),
            failed,
        ]);
        let own = [source("own", false), source("names", true)];
        let urls: Vec<(String, bool, bool)> = answer_sources(own, &cat)
            .into_iter()
            .map(|s| (s.url, s.stale, s.error.is_some()))
            .collect();
        assert_eq!(
            urls,
            [
                ("own".to_owned(), false, false),
                ("names".to_owned(), true, false),
                ("items".to_owned(), true, false),
                ("map".to_owned(), false, true),
            ]
        );
    }

    #[test]
    fn hero_filter_names_the_hero() {
        let cat = Catalog::test(&[(1, "Infernus")]);
        let found = |hero| hero_filter(&cat, hero).map_err(|p| p.message());
        assert_eq!(
            found(Some("inf")),
            Ok(("&hero_ids=1".to_owned(), Some("Infernus".to_owned())))
        );
        assert_eq!(found(None), Ok((String::new(), None)));
        assert!(found(Some("nobody")).is_err());
    }

    #[test]
    fn query_starts_on_the_hour() {
        let filters = Filters {
            min_badge: Some(71),
            max_badge: Some(116),
            days: Some(2),
        };
        // 2026-11-27 13:59:59 UTC: the window starts at 13:00 two days before.
        assert_eq!(
            filters.query_at(1_795_787_999),
            "&min_average_badge=71&max_average_badge=116&min_unix_timestamp=1795611600"
        );
        assert_eq!(Filters::default().query_at(1_795_787_999), "");
    }
}
