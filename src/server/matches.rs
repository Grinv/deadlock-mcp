//! The match tools: summary, one player in a match, lanes.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{DeadlockServer, input_schema, parse, run};
use crate::tools::matches;

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct MatchArgs {
    /// The match id, e.g. from `player_overview`.
    #[garde(range(min = 1))]
    pub match_id: u64,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct MatchPlayerArgs {
    /// The match id, e.g. from `player_overview`.
    #[garde(range(min = 1))]
    pub match_id: u64,
    /// The account id (Steam32 id) of a player in that match.
    #[garde(range(min = 1))]
    pub account_id: u32,
}

#[tool_router(router = match_tools, vis = "pub(super)")]
impl DeadlockServer {
    /// A match's mode, length and winner, and every player's hero, lane, K/D/A, souls, damage and own result.
    #[tool(
        title = "Match summary",
        input_schema = input_schema::<MatchArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn match_summary(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<matches::Summary>, CallToolResult> {
        let args: MatchArgs = parse(raw)?;
        run(move || matches::summary(args.match_id)).await
    }

    /// One player's match in depth: result and why, net worth over time, souls by source, unspent souls, deaths
    /// with killers, positions and the nearest objective, purchases and permanent buffs.
    #[tool(
        title = "Player in a match",
        input_schema = input_schema::<MatchPlayerArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn match_player(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<matches::PlayerReport>, CallToolResult> {
        let args: MatchPlayerArgs = parse(raw)?;
        run(move || matches::player(args.match_id, args.account_id)).await
    }

    /// The three lanes of a match: who laned against whom and the soul lead every 3 minutes up to 15:00.
    #[tool(
        title = "Match lanes",
        input_schema = input_schema::<MatchArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn match_lanes(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<matches::MatchLanes>, CallToolResult> {
        let args: MatchArgs = parse(raw)?;
        run(move || matches::lanes(args.match_id)).await
    }
}
