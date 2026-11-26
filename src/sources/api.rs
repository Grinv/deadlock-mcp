//! Deadlock API endpoints (<https://api.deadlock-api.com/docs>) and how long each answer stays fresh.

use std::time::Duration;

use serde_json::Value;

use crate::http::{Fresh, Source, get_json};

type Fetched = crate::problem::Result<(Value, Source)>;

const API: &str = "https://api.deadlock-api.com";
const HOUR: u64 = 3600;
/// The catalog changes with game patches; a stale copy is revalidated with its `ETag`.
const ASSETS: Fresh = Fresh::For(Duration::from_secs(24 * HOUR));

pub fn heroes() -> Fetched {
    get_json(&format!("{API}/v1/assets/heroes"), ASSETS)
}

pub fn items() -> Fetched {
    get_json(&format!("{API}/v1/assets/items"), ASSETS)
}

pub fn ranks() -> Fetched {
    get_json(&format!("{API}/v1/assets/ranks"), ASSETS)
}

/// A finished match never changes.
pub fn match_metadata(match_id: u64) -> Fetched {
    get_json(
        &format!("{API}/v1/matches/{match_id}/metadata"),
        Fresh::Forever,
    )
}

pub fn match_history(account_id: u32) -> Fetched {
    get_json(
        &format!("{API}/v1/players/{account_id}/match-history"),
        Fresh::For(Duration::from_secs(300)),
    )
}

pub fn rank(account_id: u32) -> Fetched {
    get_json(
        &format!("{API}/v1/players/{account_id}/rank"),
        Fresh::For(Duration::from_secs(HOUR)),
    )
}

/// Steam profiles (name, profile URL) of up to 1000 accounts.
pub fn steam_profiles(account_ids: &[u32]) -> Fetched {
    let ids: Vec<String> = account_ids.iter().map(u32::to_string).collect();
    get_json(
        &format!("{API}/v1/players/steam?account_ids={}", ids.join(",")),
        Fresh::For(Duration::from_secs(24 * HOUR)),
    )
}

pub fn steam_search(query: &str) -> Fetched {
    get_json(
        &format!(
            "{API}/v1/players/steam-search?search_query={}",
            crate::http::encode(query)
        ),
        Fresh::For(Duration::from_secs(HOUR)),
    )
}
