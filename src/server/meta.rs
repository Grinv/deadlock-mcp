//! The meta tools: hero statistics, builds, patches and the leaderboard.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{DeadlockServer, default_ten, input_schema, parse, run};
use crate::tools::meta::{HeroSort, Region};
use crate::tools::{Filters, game, meta};

const fn default_hundred() -> u32 {
    100
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
    /// Only matches whose players' average rank badge is at least this: tier * 10 + subrank, e.g.
    /// 116 = Eternus VI, 0 = unranked. A player's badge is behind `player_overview`'s rank.
    #[garde(inner(range(max = 116)))]
    pub min_badge: Option<u8>,
    /// Only matches whose average rank badge is at most this (same scale as `min_badge`).
    #[garde(inner(range(max = 116)))]
    pub max_badge: Option<u8>,
    /// Only matches of the last this many days; the last 30 days when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
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
    /// Only matches whose players' average rank badge is at least this: tier * 10 + subrank, e.g.
    /// 116 = Eternus VI, 0 = unranked. A player's badge is behind `player_overview`'s rank.
    #[garde(inner(range(max = 116)))]
    pub min_badge: Option<u8>,
    /// Only matches whose average rank badge is at most this (same scale as `min_badge`).
    #[garde(inner(range(max = 116)))]
    pub max_badge: Option<u8>,
    /// Only matches of the last this many days; the last 30 days when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
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
    /// Only matches whose players' average rank badge is at least this: tier * 10 + subrank, e.g.
    /// 116 = Eternus VI, 0 = unranked. A player's badge is behind `player_overview`'s rank.
    #[garde(inner(range(max = 116)))]
    pub min_badge: Option<u8>,
    /// Only matches whose average rank badge is at most this (same scale as `min_badge`).
    #[garde(inner(range(max = 116)))]
    pub max_badge: Option<u8>,
    /// Only matches of the last this many days; the last 30 days when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
    /// Leave out pairs with fewer matches.
    #[serde(default = "default_hundred")]
    #[garde(range(min = 1))]
    pub min_matches: u32,
    /// How many heroes per list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct AbilityOrderArgs {
    /// The hero: a name (or a unique part of it) or a hero id.
    #[garde(length(chars, min = 1, max = 50))]
    pub hero: String,
    /// Only matches whose players' average rank badge is at least this: tier * 10 + subrank, e.g.
    /// 116 = Eternus VI, 0 = unranked. A player's badge is behind `player_overview`'s rank.
    #[garde(inner(range(max = 116)))]
    pub min_badge: Option<u8>,
    /// Only matches whose average rank badge is at most this (same scale as `min_badge`).
    #[garde(inner(range(max = 116)))]
    pub max_badge: Option<u8>,
    /// Only matches of the last this many days; the last 30 days when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
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

fn filters(min_badge: Option<u8>, max_badge: Option<u8>, days: Option<u32>) -> Filters {
    Filters {
        min_badge,
        max_badge,
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
        let f = filters(a.min_badge, a.max_badge, a.days);
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
        let f = filters(a.min_badge, a.max_badge, a.days);
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
        let f = filters(a.min_badge, a.max_badge, a.days);
        run(move || meta::hero_counters(&a.hero, &f, a.min_matches, a.same_lane, a.limit)).await
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
        let f = filters(a.min_badge, a.max_badge, a.days);
        run(move || meta::hero_synergies(&a.hero, &f, a.min_matches, a.same_lane, a.limit)).await
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
        let f = filters(a.min_badge, a.max_badge, a.days);
        run(move || meta::ability_order(&a.hero, &f, a.min_matches, a.limit)).await
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
        run(move || game::patches(a.link.as_deref(), crate::tools::take(a.limit))).await
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
