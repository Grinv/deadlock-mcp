//! Tool logic and answer types; `server.rs` only routes MCP calls here.

pub mod game;
pub mod matches;
pub mod meta;
pub mod player;

use std::fmt::Write as _;

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
        let mut q = String::new();
        if let Some(b) = self.min_badge {
            let _ = write!(q, "&min_average_badge={b}");
        }
        if let Some(b) = self.max_badge {
            let _ = write!(q, "&max_average_badge={b}");
        }
        if let Some(days) = self.days {
            let now = crate::http::unix_now();
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

/// `count` as a `usize` for `take`.
pub fn take(count: u32) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}
