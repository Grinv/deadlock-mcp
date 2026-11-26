//! Deadlock API endpoints (<https://api.deadlock-api.com/docs>), typed by `contracts`, and how
//! long each answer stays fresh.

use std::time::Duration;

use crate::contracts::{api, valve};
use crate::http::{Fresh, Source, get_json};
use crate::problem::Result;

const API: &str = "https://api.deadlock-api.com";
const HOUR: u64 = 3600;
/// Assets of one game version; the API may still fix its extraction, so revalidate by `ETag`.
const ASSETS: Fresh = Fresh::For(Duration::from_secs(24 * HOUR));

/// Game versions the API has extracted assets for, oldest first; the last is the newest. A new
/// game patch shows up here only once the API has processed it.
pub fn client_versions() -> Result<(Vec<u32>, Source)> {
    get_json(
        &format!("{API}/v1/assets/client-versions"),
        Fresh::For(Duration::from_secs(600)),
    )
}

pub fn heroes(version: u32) -> Result<(Vec<api::Hero>, Source)> {
    get_json(
        &format!("{API}/v1/assets/heroes?client_version={version}"),
        ASSETS,
    )
}

pub fn items(version: u32) -> Result<(Vec<api::Item>, Source)> {
    get_json(
        &format!("{API}/v1/assets/items?client_version={version}"),
        ASSETS,
    )
}

pub fn ranks(version: u32) -> Result<(Vec<api::Rank>, Source)> {
    get_json(
        &format!("{API}/v1/assets/ranks?client_version={version}"),
        ASSETS,
    )
}

/// Game constants, among them `lane_info` (lane names and colors by `assigned_lane`).
pub fn generic_data(version: u32) -> Result<(api::GenericData, Source)> {
    get_json(
        &format!("{API}/v1/assets/generic-data?client_version={version}"),
        ASSETS,
    )
}

/// A finished match never changes. The API declares this response as a bare object; it is
/// Valve's `CMsgMatchMetaDataContents` plus a few extra fields, which are ignored here.
pub fn match_metadata(match_id: u64) -> Result<(valve::CMsgMatchMetaDataContents, Source)> {
    get_json(
        &format!("{API}/v1/matches/{match_id}/metadata"),
        Fresh::Forever,
    )
}

pub fn match_history(account_id: u32) -> Result<(Vec<api::PlayerMatchHistoryEntry>, Source)> {
    get_json(
        &format!("{API}/v1/players/{account_id}/match-history"),
        Fresh::For(Duration::from_secs(300)),
    )
}

pub fn rank(account_id: u32) -> Result<(api::RankResponse, Source)> {
    get_json(
        &format!("{API}/v1/players/{account_id}/rank"),
        Fresh::For(Duration::from_secs(HOUR)),
    )
}

/// Steam profiles (name, profile URL) of up to 1000 accounts.
pub fn steam_profiles(account_ids: &[u32]) -> Result<(Vec<api::SteamProfile>, Source)> {
    let ids: Vec<String> = account_ids.iter().map(u32::to_string).collect();
    get_json(
        &format!("{API}/v1/players/steam?account_ids={}", ids.join(",")),
        Fresh::For(Duration::from_secs(24 * HOUR)),
    )
}

pub fn steam_search(query: &str) -> Result<(Vec<api::SteamProfile>, Source)> {
    get_json(
        &format!(
            "{API}/v1/players/steam-search?search_query={}",
            crate::http::encode(query)
        ),
        Fresh::For(Duration::from_secs(HOUR)),
    )
}
