//! The player tools: search, overview, per-hero stats, mates, opponents, death map, ingest status.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{DeadlockServer, default_ten, input_schema, parse, run};
use crate::tools::player;

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PlayerSearchArgs {
    /// A Steam name, an account id, a `SteamID64` or a steamcommunity.com/profiles/ link.
    #[garde(length(chars, min = 1, max = 100))]
    pub query: String,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PlayerOverviewArgs {
    /// The player's account id (Steam32 id), from `player_search`.
    #[garde(range(min = 1))]
    pub account_id: u32,
    /// How many recent matches to list.
    #[serde(default = "default_recent")]
    #[garde(range(min = 1, max = 50))]
    pub recent: u32,
}

const fn default_recent() -> u32 {
    10
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct AccountArgs {
    /// The player's account id (Steam32 id), from `player_search`.
    #[garde(range(min = 1))]
    pub account_id: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PlayerHeroesArgs {
    /// The player's account id (Steam32 id), from `player_search`.
    #[garde(range(min = 1))]
    pub account_id: u32,
    /// Only matches of the last this many days; all the API has when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
    /// How many heroes to list, most played first.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

const fn default_two() -> u32 {
    2
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PlayerMatesArgs {
    /// The player's account id (Steam32 id), from `player_search`.
    #[garde(range(min = 1))]
    pub account_id: u32,
    /// Only mates who queued in the same party (friends), not random teammates.
    #[serde(default)]
    #[garde(skip)]
    pub same_party: bool,
    /// Leave out mates with fewer matches together.
    #[serde(default = "default_two")]
    #[garde(range(min = 1, max = 1000))]
    pub min_matches: u32,
    /// Only matches of the last this many days; all the API has when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
    /// How many mates to list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PlayerEnemiesArgs {
    /// The player's account id (Steam32 id), from `player_search`.
    #[garde(range(min = 1))]
    pub account_id: u32,
    /// Leave out opponents met fewer times.
    #[serde(default = "default_two")]
    #[garde(range(min = 1, max = 1000))]
    pub min_matches: u32,
    /// Only matches of the last this many days; all the API has when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
    /// How many opponents per list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct PlayerDeathsMapArgs {
    /// The player's account id (Steam32 id), from `player_search`.
    #[garde(range(min = 1))]
    pub account_id: u32,
    /// Only matches on this hero: a name (or a unique part of it) or a hero id.
    #[garde(inner(length(chars, min = 1, max = 50)))]
    pub hero: Option<String>,
    /// Only matches of the last this many days; the last 30 days when left out.
    #[garde(inner(range(min = 1, max = 365)))]
    pub days: Option<u32>,
    /// How many hotspots per list.
    #[serde(default = "default_ten")]
    #[garde(range(min = 1, max = 50))]
    pub limit: u32,
}

#[tool_router(router = player_tools, vis = "pub(super)")]
impl DeadlockServer {
    /// Find a player's account id by Steam name, `SteamID64` or profile link.
    #[tool(
        title = "Find a player",
        input_schema = input_schema::<PlayerSearchArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn player_search(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<player::Search>, CallToolResult> {
        let args: PlayerSearchArgs = parse(raw)?;
        run(move || player::search(&args.query)).await
    }

    /// A player's rank, win rate over counted matches, most played heroes and recent matches.
    #[tool(
        title = "Player overview",
        input_schema = input_schema::<PlayerOverviewArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn player_overview(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<player::Overview>, CallToolResult> {
        let args: PlayerOverviewArgs = parse(raw)?;
        let recent = usize::try_from(args.recent).unwrap_or(usize::MAX);
        run(move || player::overview(args.account_id, recent)).await
    }

    /// A player's stats per hero: matches, win rate, K/D/A per match, souls and damage per minute.
    #[tool(
        title = "Player's heroes",
        input_schema = input_schema::<PlayerHeroesArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn player_heroes(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<player::PlayerHeroes>, CallToolResult> {
        let a: PlayerHeroesArgs = parse(raw)?;
        run(move || player::heroes(a.account_id, a.days, a.limit)).await
    }

    /// Who a player plays with: matches together and the win rate, with Steam names.
    #[tool(
        title = "Player's teammates",
        input_schema = input_schema::<PlayerMatesArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn player_mates(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<player::PlayerMates>, CallToolResult> {
        let a: PlayerMatesArgs = parse(raw)?;
        run(move || player::mates(a.account_id, a.same_party, a.days, a.min_matches, a.limit)).await
    }

    /// The opponents a player met most and lost to most, with Steam names.
    #[tool(
        title = "Player's opponents",
        input_schema = input_schema::<PlayerEnemiesArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn player_enemies(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<player::Enemies>, CallToolResult> {
        let a: PlayerEnemiesArgs = parse(raw)?;
        run(move || player::enemies(a.account_id, a.days, a.min_matches, a.limit)).await
    }

    /// Where on the map a player dies and kills most: hotspots in `match_player`'s map coordinates, with the
    /// objective nearby.
    #[tool(
        title = "Player's death map",
        input_schema = input_schema::<PlayerDeathsMapArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn player_deaths_map(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<player::DeathsMap>, CallToolResult> {
        let a: PlayerDeathsMapArgs = parse(raw)?;
        run(move || player::deaths_map(a.account_id, a.hero.as_deref(), a.days, a.limit)).await
    }

    /// Whether the player runs the API's ingest tool, which sends their matches as they play.
    #[tool(
        title = "Match ingestion status",
        input_schema = input_schema::<AccountArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn ingest_status(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<player::IngestStatus>, CallToolResult> {
        let a: AccountArgs = parse(raw)?;
        run(move || player::ingest_status(a.account_id)).await
    }
}
