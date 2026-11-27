//! Tool logic and answer types; `src/server/` only routes MCP calls here.

pub mod game;
pub mod matches;
pub mod meta;
pub mod player;

use std::fmt::Write as _;

use crate::domain::assets::Catalog;
use crate::http::Source;

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

/// The sources of an answer built with the catalog: `source` and the catalog's degraded ones.
pub fn answer_sources(source: Source, cat: &Catalog) -> Vec<Source> {
    std::iter::once(source)
        .chain(cat.degraded_sources())
        .collect()
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

/// An API id as the catalog's `u32`; ids out of range become 0, which names no hero or item.
pub fn id32(id: i64) -> u32 {
    u32::try_from(id).unwrap_or_default()
}

/// `count` as a `usize` for `take`.
pub fn take(count: u32) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer lists its own source first, then only the catalog's stale or failed ones.
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
        let urls: Vec<(String, bool, bool)> = answer_sources(source("own", false), &cat)
            .into_iter()
            .map(|s| (s.url, s.stale, s.error.is_some()))
            .collect();
        assert_eq!(
            urls,
            [
                ("own".to_owned(), false, false),
                ("items".to_owned(), true, false),
                ("map".to_owned(), false, true),
            ]
        );
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
