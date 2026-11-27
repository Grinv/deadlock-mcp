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

/// End-of-match awards: id, name and the stat each tracks.
pub fn accolades(version: u32) -> Result<(Vec<api::Accolade>, Source)> {
    get_json(
        &format!("{API}/v1/assets/accolades?client_version={version}"),
        ASSETS,
    )
}

/// The minimap: its radius, the objectives' marker positions and the lanes' zip lines.
pub fn map(version: u32) -> Result<(api::MapData, Source)> {
    get_json(
        &format!("{API}/v1/assets/map?client_version={version}"),
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

/// Player stats change with every match the API ingests.
const PLAYER: Fresh = Fresh::For(Duration::from_secs(600));
/// Analytics are aggregated over many matches; the API itself caches them for an hour.
const ANALYTICS: Fresh = Fresh::For(Duration::from_secs(HOUR));

/// Per-hero stats of one player; `filters` is `&key=value...` (time range).
pub fn player_hero_stats(account_id: u32, filters: &str) -> Result<(Vec<api::HeroStats>, Source)> {
    get_json(
        &format!("{API}/v1/players/hero-stats?account_ids={account_id}{filters}"),
        PLAYER,
    )
}

pub fn mate_stats(account_id: u32, filters: &str) -> Result<(Vec<api::MateStats>, Source)> {
    get_json(
        &format!("{API}/v1/players/{account_id}/mate-stats?game_mode=normal{filters}"),
        PLAYER,
    )
}

pub fn enemy_stats(account_id: u32, filters: &str) -> Result<(Vec<api::EnemyStats>, Source)> {
    get_json(
        &format!("{API}/v1/players/{account_id}/enemy-stats?game_mode=normal{filters}"),
        PLAYER,
    )
}

/// Kills and deaths on a raster of the map; with `account_ids`, that player's own.
pub fn kill_death_stats(filters: &str) -> Result<(Vec<api::KillDeathStats>, Source)> {
    get_json(
        &format!("{API}/v1/analytics/kill-death-stats?game_mode=normal{filters}"),
        PLAYER,
    )
}

pub fn ingest_status(account_id: u32) -> Result<(api::IngestStatusResponse, Source)> {
    get_json(
        &format!("{API}/v1/players/{account_id}/ingest-status"),
        Fresh::For(Duration::from_secs(60)),
    )
}

pub fn builds(filters: &str) -> Result<(Vec<api::Build>, Source)> {
    get_json(
        &format!("{API}/v1/builds?only_latest=true{filters}"),
        ANALYTICS,
    )
}

pub fn hero_stats(filters: &str) -> Result<(Vec<api::AnalyticsHeroStats>, Source)> {
    get_json(
        &format!("{API}/v1/analytics/hero-stats?game_mode=normal{filters}"),
        ANALYTICS,
    )
}

pub fn item_stats(filters: &str) -> Result<(Vec<api::ItemStats>, Source)> {
    get_json(
        &format!("{API}/v1/analytics/item-stats?game_mode=normal{filters}"),
        ANALYTICS,
    )
}

/// Every hero against every enemy hero; the endpoint has no hero filter.
pub fn hero_counter_stats(filters: &str) -> Result<(Vec<api::HeroCounterStats>, Source)> {
    get_json(
        &format!("{API}/v1/analytics/hero-counter-stats?game_mode=normal{filters}"),
        ANALYTICS,
    )
}

/// Every pair of heroes on one team; the endpoint has no hero filter.
pub fn hero_synergy_stats(filters: &str) -> Result<(Vec<api::HeroSynergyStats>, Source)> {
    get_json(
        &format!("{API}/v1/analytics/hero-synergy-stats?game_mode=normal{filters}"),
        ANALYTICS,
    )
}

pub fn ability_order_stats(
    hero_id: u32,
    filters: &str,
) -> Result<(Vec<api::AnalyticsAbilityOrderStats>, Source)> {
    get_json(
        &format!("{API}/v1/analytics/ability-order-stats?hero_id={hero_id}{filters}"),
        ANALYTICS,
    )
}

/// The average player's stats at each stats snapshot of a match (every 3 minutes up to 15:00,
/// then every 5), in seconds of game time rather than shares of the match.
pub fn performance_curve(filters: &str) -> Result<(Vec<api::PlayerPerformanceCurvePoint>, Source)> {
    get_json(
        &format!(
            "{API}/v1/analytics/player-performance-curve?game_mode=normal&resolution=0{filters}"
        ),
        ANALYTICS,
    )
}

/// Percentiles of per-match player stats; without `bucket` one object, metric by metric.
pub fn stat_metrics(filters: &str) -> Result<(api::PlayerStatsMetrics, Source)> {
    get_json(
        &format!("{API}/v1/analytics/player-stats/metrics?game_mode=normal{filters}"),
        ANALYTICS,
    )
}

/// Patch notes from the official forum changelog and Steam news, newest first.
pub fn patches() -> Result<(Vec<api::FeedItem>, Source)> {
    get_json(&format!("{API}/v2/patches"), ANALYTICS)
}

/// Valve updates the leaderboard once an hour.
pub fn leaderboard(
    region: api::Region,
    hero_id: Option<u32>,
) -> Result<(api::Leaderboard, Source)> {
    let hero = hero_id.map_or_default(|h| format!("/{h}"));
    get_json(&format!("{API}/v1/leaderboard/{region}{hero}"), ANALYTICS)
}
