//! Wire types generated from the pinned upstream contracts in `contracts/`; regenerate with
//! `cargo xtask contracts` (see `contracts/SOURCES.md`). Only the server's own modules map them to
//! answers, so an upstream change shows up as a compile or parse error here, not as a silent zero.

/// Deadlock API responses, from its `OpenAPI` spec.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    dead_code,
    dead_code_pub_in_binary,
    unused
)]
pub mod api;
/// Valve's protobuf messages; the Deadlock API serves match metadata as these in JSON.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    dead_code,
    dead_code_pub_in_binary,
    unused
)]
pub mod valve;

/// Real (trimmed) API answers parse into the generated types: catches a generator setting that
/// produces types stricter than the published schema, which no compile error shows.
#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::{api, valve};

    fn parse<T: serde::de::DeserializeOwned>(json: &str) -> T {
        let de = &mut serde_json::Deserializer::from_str(json);
        serde_path_to_error::deserialize(de).unwrap_or_else(|e| panic!("{}: {}", e.path(), e))
    }

    #[test]
    fn api_fixtures_parse() {
        let items: Vec<api::Item> = parse(include_str!("../../contracts/fixtures/items.json"));
        assert_matches!(items[0], api::Item::Ability { .. });
        assert_matches!(items[1], api::Item::Weapon { .. });
        assert_matches!(items[2], api::Item::Upgrade { .. });
        let _: Vec<api::Hero> = parse(include_str!("../../contracts/fixtures/heroes.json"));
        let _: Vec<api::Rank> = parse(include_str!("../../contracts/fixtures/ranks.json"));
        let data: api::GenericData =
            parse(include_str!("../../contracts/fixtures/generic_data.json"));
        assert!(!data.lane_info.is_empty());
        let history: Vec<api::PlayerMatchHistoryEntry> =
            parse(include_str!("../../contracts/fixtures/match_history.json"));
        assert!(!history.is_empty());
        let _: api::RankResponse = parse(include_str!("../../contracts/fixtures/rank.json"));
        let _: Vec<api::HeroStats> = parse(include_str!(
            "../../contracts/fixtures/player_hero_stats.json"
        ));
        let _: Vec<api::MateStats> =
            parse(include_str!("../../contracts/fixtures/mate_stats.json"));
        let _: Vec<api::EnemyStats> =
            parse(include_str!("../../contracts/fixtures/enemy_stats.json"));
        let _: Vec<api::KillDeathStats> = parse(include_str!(
            "../../contracts/fixtures/kill_death_stats.json"
        ));
        let _: api::IngestStatusResponse =
            parse(include_str!("../../contracts/fixtures/ingest_status.json"));
        let _: Vec<api::Build> = parse(include_str!("../../contracts/fixtures/builds.json"));
        let _: Vec<api::AnalyticsHeroStats> = parse(include_str!(
            "../../contracts/fixtures/analytics_hero_stats.json"
        ));
        let _: Vec<api::ItemStats> =
            parse(include_str!("../../contracts/fixtures/item_stats.json"));
        let _: Vec<api::HeroCounterStats> = parse(include_str!(
            "../../contracts/fixtures/hero_counter_stats.json"
        ));
        let _: Vec<api::HeroSynergyStats> = parse(include_str!(
            "../../contracts/fixtures/hero_synergy_stats.json"
        ));
        let _: Vec<api::AnalyticsAbilityOrderStats> = parse(include_str!(
            "../../contracts/fixtures/ability_order_stats.json"
        ));
        let patches: Vec<api::FeedItem> =
            parse(include_str!("../../contracts/fixtures/patches.json"));
        assert_matches!(patches[0], api::FeedItem::ForumPatch { .. });
        assert_matches!(patches[1], api::FeedItem::SteamNews { .. });
        let accolades: Vec<api::Accolade> =
            parse(include_str!("../../contracts/fixtures/accolades.json"));
        assert!(!accolades.is_empty());
        let map: api::MapData = parse(include_str!("../../contracts/fixtures/map.json"));
        assert!(map.objective_positions.contains_key("team1_tier1_1"));
        let curve: Vec<api::PlayerPerformanceCurvePoint> = parse(include_str!(
            "../../contracts/fixtures/performance_curve.json"
        ));
        assert_eq!(curve[0].game_time, 180);
        let lane: Vec<api::LaneSoulCurve> = parse(include_str!(
            "../../contracts/fixtures/lane_soul_curve.json"
        ));
        assert!(
            lane[0]
                .stats
                .contains_key(&api::LaneSoulCurveStatsKey::PlayerDamage)
        );
        let lane: Vec<api::LaneMatchupStats> = parse(include_str!(
            "../../contracts/fixtures/lane_matchup_stats.json"
        ));
        assert_eq!(lane[0].hero_ids, [11, 76]);
        let flow: api::ItemFlowStats = parse(include_str!(
            "../../contracts/fixtures/item_flow_stats.json"
        ));
        assert_eq!(flow.reached_per_column.len(), 4);
        assert!(flow.nodes.iter().any(|n| n.column == 3));
        let metrics: api::PlayerStatsMetrics =
            parse(include_str!("../../contracts/fixtures/stat_metrics.json"));
        assert!(metrics.contains_key(&api::PlayerStatsMetricsKey::NetWorthPerMin));
        let board: api::Leaderboard =
            parse(include_str!("../../contracts/fixtures/leaderboard.json"));
        assert!(!board.entries.is_empty());
    }

    #[test]
    fn match_metadata_parses() {
        let meta: valve::CMsgMatchMetaDataContents =
            parse(include_str!("../../contracts/fixtures/match_metadata.json"));
        let info = meta.match_info.expect("match_info");
        assert_eq!(info.match_id(), 143_768_397);
        assert_eq!(info.players.len(), 12);
        assert_eq!(
            info.match_mode(),
            valve::ECitadelMatchMode::KECitadelMatchModeRanked
        );
    }
}
