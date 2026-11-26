//! Wire types generated from the pinned upstream contracts in `contracts/`; regenerate with
//! `cargo xtask contracts` (see `contracts/SOURCES.md`). Only the server's own modules map them to
//! answers, so an upstream change shows up as a compile or parse error here, not as a silent zero.

/// Deadlock API responses, from its `OpenAPI` spec.
#[allow(clippy::all, clippy::pedantic, clippy::nursery, dead_code, unused)]
pub mod api;
/// Valve's protobuf messages; the Deadlock API serves match metadata as these in JSON.
#[allow(clippy::all, clippy::pedantic, clippy::nursery, dead_code, unused)]
pub mod valve;

/// Real (trimmed) API answers parse into the generated types: catches a generator setting that
/// produces types stricter than the published schema, which no compile error shows.
#[cfg(test)]
mod tests {
    use super::{api, valve};

    fn parse<T: serde::de::DeserializeOwned>(json: &str) -> T {
        let de = &mut serde_json::Deserializer::from_str(json);
        serde_path_to_error::deserialize(de).unwrap_or_else(|e| panic!("{}: {}", e.path(), e))
    }

    #[test]
    fn api_fixtures_parse() {
        let items: Vec<api::Item> = parse(include_str!("../../contracts/fixtures/items.json"));
        assert!(matches!(items[0], api::Item::Ability { .. }));
        assert!(matches!(items[1], api::Item::Weapon { .. }));
        assert!(matches!(items[2], api::Item::Upgrade { .. }));
        let _: Vec<api::Hero> = parse(include_str!("../../contracts/fixtures/heroes.json"));
        let _: Vec<api::Rank> = parse(include_str!("../../contracts/fixtures/ranks.json"));
        let data: api::GenericData =
            parse(include_str!("../../contracts/fixtures/generic_data.json"));
        assert!(!data.lane_info.is_empty());
        let history: Vec<api::PlayerMatchHistoryEntry> =
            parse(include_str!("../../contracts/fixtures/match_history.json"));
        assert!(!history.is_empty());
        let _: api::RankResponse = parse(include_str!("../../contracts/fixtures/rank.json"));
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
