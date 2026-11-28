//! The meta tools: hero statistics, lane matchups, builds, patches and the leaderboard.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{DeadlockServer, default_ten, default_window_days, input_schema, parse, run};
use crate::contracts::api::{PlayerStatsMetricsKey, Region};
use crate::tools::meta::HeroSort;
use crate::tools::{MetaFilters, game, meta};

const fn default_hundred() -> u32 {
    100
}

/// Duo against duo meets far less often than hero against hero.
const fn default_thirty() -> u32 {
    30
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct BuildsArgs {
    /// The hero: a name (or a unique part of it) or a hero id.
    #[garde(length(chars, min = 1, max = 50))]
    pub hero: String,
    /// Only builds whose name contains this text.
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub search: Option<String>,
    /// Only builds written in this language; all languages when left out.
    #[garde(skip)]
    pub language: Option<crate::contracts::api::BuildLanguage>,
    /// How many builds to list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct HeroStatsArgs {
    /// Only matches whose players' average rank is at least this rank, named as `player_overview`
    /// names it: "Ascendant" (from Ascendant I), "Eternus III"; "Obscurus" is unranked.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub min_rank: Option<String>,
    /// Only matches whose average rank is at most this rank: "Ascendant" is up to Ascendant VI.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub max_rank: Option<String>,
    /// Only matches of the last this many days.
    #[serde(default = "default_window_days")]
    #[garde(range(min = 1, max = 365))]
    pub days: u32,
    /// Leave out heroes with fewer matches.
    #[serde(default = "default_hundred")]
    #[garde(range(min = 1))]
    pub min_matches: u32,
    /// `win_rate` (default) or `pick_rate`.
    #[serde(default)]
    #[garde(skip)]
    pub sort: HeroSort,
    /// How many heroes to list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct HeroItemsArgs {
    /// The hero: a name (or a unique part of it) or a hero id.
    #[garde(length(chars, min = 1, max = 50))]
    pub hero: String,
    /// Only matches against at least one of these heroes (names or ids).
    #[serde(default)]
    #[garde(length(max = 5), inner(length(chars, min = 1, max = 50)))]
    pub enemy_heroes: Vec<String>,
    /// Only matches whose players' average rank is at least this rank, named as `player_overview`
    /// names it: "Ascendant" (from Ascendant I), "Eternus III"; "Obscurus" is unranked.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub min_rank: Option<String>,
    /// Only matches whose average rank is at most this rank: "Ascendant" is up to Ascendant VI.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub max_rank: Option<String>,
    /// Only matches of the last this many days.
    #[serde(default = "default_window_days")]
    #[garde(range(min = 1, max = 365))]
    pub days: u32,
    /// Leave out items bought in fewer matches.
    #[serde(default = "default_hundred")]
    #[garde(range(min = 1))]
    pub min_matches: u32,
    /// How many items to list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct HeroPairArgs {
    /// The hero: a name (or a unique part of it) or a hero id.
    #[garde(length(chars, min = 1, max = 50))]
    pub hero: String,
    /// Only pairs that shared a lane (lane opponents, lane partners); all matches when false.
    #[serde(default)]
    #[garde(skip)]
    pub same_lane: bool,
    /// Only matches whose players' average rank is at least this rank, named as `player_overview`
    /// names it: "Ascendant" (from Ascendant I), "Eternus III"; "Obscurus" is unranked.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub min_rank: Option<String>,
    /// Only matches whose average rank is at most this rank: "Ascendant" is up to Ascendant VI.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub max_rank: Option<String>,
    /// Only matches of the last this many days.
    #[serde(default = "default_window_days")]
    #[garde(range(min = 1, max = 365))]
    pub days: u32,
    /// Leave out pairs with fewer matches.
    #[serde(default = "default_hundred")]
    #[garde(range(min = 1))]
    pub min_matches: u32,
    /// How many heroes per list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
    /// Also place these heroes among all pairs, e.g. a match's lane opponents for
    /// `hero_counters` or the lane partner for `hero_synergies`: names or ids.
    #[serde(default)]
    #[garde(length(max = 11), inner(length(chars, min = 1, max = 50)))]
    pub heroes: Vec<String>,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct LaneMatchupsArgs {
    /// The two heroes of the lane duo: names (or unique parts of them) or hero ids.
    #[garde(length(min = 2, max = 2), inner(length(chars, min = 1, max = 50)))]
    pub heroes: Vec<String>,
    /// The two heroes they laned against; leave out for the duo against any duo.
    #[serde(default)]
    #[garde(length(max = 2), inner(length(chars, min = 1, max = 50)))]
    pub enemy_heroes: Vec<String>,
    /// Only matches whose players' average rank is at least this rank, named as `player_overview`
    /// names it: "Ascendant" (from Ascendant I), "Eternus III"; "Obscurus" is unranked.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub min_rank: Option<String>,
    /// Only matches whose average rank is at most this rank: "Ascendant" is up to Ascendant VI.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub max_rank: Option<String>,
    /// Only matches of the last this many days.
    #[serde(default = "default_window_days")]
    #[garde(range(min = 1, max = 365))]
    pub days: u32,
    /// Answer with `matches` 0 and no points when the duos met in fewer lanes. Few lanes give a
    /// rough mean: say how many the answer rests on.
    #[serde(default = "default_thirty")]
    #[garde(range(min = 1))]
    pub min_matches: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct AbilityOrderArgs {
    /// The hero: a name (or a unique part of it) or a hero id.
    #[garde(length(chars, min = 1, max = 50))]
    pub hero: String,
    /// Only matches whose players' average rank is at least this rank, named as `player_overview`
    /// names it: "Ascendant" (from Ascendant I), "Eternus III"; "Obscurus" is unranked.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub min_rank: Option<String>,
    /// Only matches whose average rank is at most this rank: "Ascendant" is up to Ascendant VI.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub max_rank: Option<String>,
    /// Only matches of the last this many days.
    #[serde(default = "default_window_days")]
    #[garde(range(min = 1, max = 365))]
    pub days: u32,
    /// Leave out orders played in fewer matches.
    #[serde(default = "default_hundred")]
    #[garde(range(min = 1))]
    pub min_matches: u32,
    /// How many orders per list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PatchesArgs {
    /// A patch's link from the list: returns that patch's notes as text instead of the list.
    #[garde(inner(length(chars, min = 1, max = 500)))]
    pub link: Option<String>,
    /// How many patches to list, newest first.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct LeaderboardArgs {
    /// The leaderboard's region.
    #[garde(skip)]
    pub region: Region,
    /// One hero's leaderboard: a name (or a unique part of it) or a hero id.
    #[garde(inner(length(chars, min = 1, max = 50)))]
    pub hero: Option<String>,
    /// How many players to list from the top.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 100))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PerformanceCurveArgs {
    /// Only matches of this hero: a name (or a unique part of it) or a hero id; all heroes when
    /// left out.
    #[garde(inner(length(chars, min = 1, max = 50)))]
    pub hero: Option<String>,
    /// Only matches whose players' average rank is at least this rank, named as `player_overview`
    /// names it: "Ascendant" (from Ascendant I), "Eternus III"; "Obscurus" is unranked.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub min_rank: Option<String>,
    /// Only matches whose average rank is at most this rank: "Ascendant" is up to Ascendant VI.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub max_rank: Option<String>,
    /// Only matches of the last this many days.
    #[serde(default = "default_window_days")]
    #[garde(range(min = 1, max = 365))]
    pub days: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct StatPercentilesArgs {
    /// Only matches of this hero: a name (or a unique part of it) or a hero id; all heroes when
    /// left out.
    #[garde(inner(length(chars, min = 1, max = 50)))]
    pub hero: Option<String>,
    /// The stats to give; when left out, the main ones (net worth per minute, kills, deaths,
    /// assists, last hits, hero damage per minute, accuracy).
    #[serde(default)]
    #[garde(length(max = 32))]
    pub metrics: Vec<PlayerStatsMetricsKey>,
    /// Only matches whose players' average rank is at least this rank, named as `player_overview`
    /// names it: "Ascendant" (from Ascendant I), "Eternus III"; "Obscurus" is unranked.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub min_rank: Option<String>,
    /// Only matches whose average rank is at most this rank: "Ascendant" is up to Ascendant VI.
    #[garde(inner(length(chars, min = 1, max = 30)))]
    pub max_rank: Option<String>,
    /// Only matches of the last this many days.
    #[serde(default = "default_window_days")]
    #[garde(range(min = 1, max = 365))]
    pub days: u32,
}

fn filters(min_rank: Option<String>, max_rank: Option<String>, days: u32) -> MetaFilters {
    MetaFilters {
        min_rank,
        max_rank,
        days,
    }
}

#[tool_router(router = meta_tools, vis = "pub(super)")]
impl DeadlockServer {
    /// Popular in-game builds of a hero: name, author, the build id to import in game, favorites.
    #[tool(
        title = "Hero builds",
        input_schema = input_schema::<BuildsArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn builds(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::HeroBuilds>, CallToolResult> {
        let a: BuildsArgs = parse(raw)?;
        run(move || meta::builds(&a.hero, a.search.as_deref(), a.language, a.limit)).await
    }

    /// Win rate, pick rate and matches per hero over all players' matches.
    #[tool(
        title = "Hero win and pick rates",
        input_schema = input_schema::<HeroStatsArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn hero_stats(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::HeroStats>, CallToolResult> {
        let a: HeroStatsArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || meta::hero_stats(&f, a.min_matches, a.sort, a.limit)).await
    }

    /// Which shop items win on a hero, optionally against given enemy heroes.
    #[tool(
        title = "Items on a hero",
        input_schema = input_schema::<HeroItemsArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn hero_items(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::HeroItems>, CallToolResult> {
        let a: HeroItemsArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || meta::hero_items(&a.hero, &a.enemy_heroes, &f, a.min_matches, a.limit)).await
    }

    /// The enemy heroes a hero wins most and least against.
    #[tool(
        title = "Hero counters",
        input_schema = input_schema::<HeroPairArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn hero_counters(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::Counters>, CallToolResult> {
        let a: HeroPairArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || {
            meta::hero_counters(&a.hero, &f, a.min_matches, a.same_lane, a.limit, &a.heroes)
        })
        .await
    }

    /// The teammates a hero wins most and least with.
    #[tool(
        title = "Hero synergies",
        input_schema = input_schema::<HeroPairArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn hero_synergies(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::Synergies>, CallToolResult> {
        let a: HeroPairArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || {
            meta::hero_synergies(&a.hero, &f, a.min_matches, a.same_lane, a.limit, &a.heroes)
        })
        .await
    }

    /// A lane duo against an enemy duo (or any duo): soul lead with its spread and damage lead at
    /// each stats snapshot, and the win rate. Upstream marks the data as subject to change.
    #[tool(
        title = "Lane matchups",
        input_schema = input_schema::<LaneMatchupsArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn lane_matchups(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::LaneMatchups>, CallToolResult> {
        let a: LaneMatchupsArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || meta::lane_matchups(&a.heroes, &a.enemy_heroes, &f, a.min_matches)).await
    }

    /// A hero's most common and best-winning ability upgrade orders.
    #[tool(
        title = "Ability upgrade orders",
        input_schema = input_schema::<AbilityOrderArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn ability_order(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::AbilityOrders>, CallToolResult> {
        let a: AbilityOrderArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || meta::ability_order(&a.hero, &f, a.min_matches, a.limit)).await
    }

    /// The average player's course of a match: net worth, K/D/A, last hits, damage and souls by
    /// source at each stats snapshot (every 3 minutes up to 15:00, then every 5).
    #[tool(
        title = "Performance curve",
        input_schema = input_schema::<PerformanceCurveArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn performance_curve(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::PerformanceCurve>, CallToolResult> {
        let a: PerformanceCurveArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || meta::performance_curve(a.hero.as_deref(), &f)).await
    }

    /// Percentiles of per-match player stats (souls per minute, K/D/A, damage, accuracy...), to
    /// place one match's numbers among all players' matches.
    #[tool(
        title = "Stat percentiles",
        input_schema = input_schema::<StatPercentilesArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn stat_percentiles(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::StatPercentiles>, CallToolResult> {
        let a: StatPercentilesArgs = parse(raw)?;
        let f = filters(a.min_rank, a.max_rank, a.days);
        run(move || meta::stat_percentiles(a.hero.as_deref(), &a.metrics, &f)).await
    }

    /// Patch notes and news: the list, or one patch's notes as text.
    #[tool(
        title = "Patch notes",
        input_schema = input_schema::<PatchesArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn patches(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<game::PatchFeed>, CallToolResult> {
        let a: PatchesArgs = parse(raw)?;
        run(move || game::patches(a.link.as_deref(), a.limit)).await
    }

    /// The top players of a region's ranked leaderboard, optionally for one hero.
    #[tool(
        title = "Leaderboard",
        input_schema = input_schema::<LeaderboardArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn leaderboard(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<meta::Leaderboard>, CallToolResult> {
        let a: LeaderboardArgs = parse(raw)?;
        run(move || meta::leaderboard(a.region, a.hero.as_deref(), a.limit)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The region is the API's own enum: parsed, sent and answered under the API's names.
    #[test]
    fn leaderboard_region_uses_the_api_names() {
        let args: LeaderboardArgs =
            serde_json::from_value(serde_json::json!({"region": "NAmerica"})).unwrap();
        assert_eq!(args.region, Region::NAmerica);
        assert_eq!(args.region.to_string(), "NAmerica");
        assert_eq!(
            serde_json::to_value(args.region).unwrap(),
            serde_json::json!("NAmerica")
        );
        assert!(
            serde_json::from_value::<LeaderboardArgs>(serde_json::json!({"region": "Mars"}))
                .is_err()
        );
    }

    /// A meta tool called without `days` reads the last 30 days, not all of the API's history.
    #[test]
    fn meta_window_defaults_to_30_days() {
        let args: HeroStatsArgs = serde_json::from_value(
            serde_json::json!({"min_rank": "ascendant", "max_rank": "Eternus"}),
        )
        .unwrap();
        assert_eq!(args.days, 30);
        // Rank names become the API's badges: a whole rank spans its six subranks.
        let cat = crate::domain::assets::Catalog::test(&[]).with_ranks(&[
            (0, "Obscurus"),
            (10, "Ascendant"),
            (11, "Eternus"),
        ]);
        let query = filters(args.min_rank, args.max_rank, args.days)
            .to_filters(&cat)
            .unwrap()
            .query();
        assert!(
            query.starts_with("&min_average_badge=101&max_average_badge=116&min_unix_timestamp="),
            "{query}"
        );
    }
}
