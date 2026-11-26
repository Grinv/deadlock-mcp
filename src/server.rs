//! The MCP surface: tools, prompts and resources. Each handler parses its arguments, runs the
//! blocking tool logic off the async runtime and turns a `Problem` into an error result.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListResourcesResult, PaginatedRequestParams,
    PromptMessage, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, prompt, prompt_handler, prompt_router, tool,
    tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::domain::assets::catalog;
use crate::problem::{Kind, Problem};
use crate::sources::wiki;
use crate::tools::{game, matches, player};

const INSTRUCTIONS: &str = "\
Deadlock (Valve's hero shooter) statistics and game knowledge.
- Players are identified by account id (the Steam32 id). Start with player_search when you only have a name, a SteamID64 or a profile link.
- player_overview lists a player's recent matches with their match ids; the match_* tools take those ids.
- A player's `result` is their own: `not_scored` when the game voided the match for them (usually a teammate left); a leaver's match is a loss even if their team won.
- Every answer lists its `sources`; when one is `stale`, the data source failed and an older copy was used, so say so.
- Hero, ability and item names come from game_lookup; mechanics and explanations from wiki_search and wiki_page (cite the page URL; the text is CC BY-NC-SA 4.0).";

fn fail(problem: &Problem) -> CallToolResult {
    let mut result = CallToolResult::error(vec![ContentBlock::text(problem.message())]);
    result.structured_content = serde_json::to_value(problem).ok();
    result
}

/// Runs blocking tool logic (HTTP, disk cache, JSON parsing) on tokio's blocking pool.
async fn run<T, F>(f: F) -> Result<Json<T>, CallToolResult>
where
    T: Send + 'static,
    F: FnOnce() -> crate::problem::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Ok(Json(value)),
        Ok(Err(problem)) => Err(fail(&problem)),
        Err(e) => Err(fail(&Problem::new(
            Kind::Upstream,
            format!("the tool crashed: {e}"),
        ))),
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct PlayerSearchArgs {
    /// A Steam name, an account id, a `SteamID64` or a steamcommunity.com/profiles/ link.
    pub query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlayerOverviewArgs {
    /// The player's account id (Steam32 id), from `player_search`.
    pub account_id: u32,
    /// How many recent matches to list (default 10, at most 50).
    pub recent: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MatchArgs {
    /// The match id, e.g. from `player_overview`.
    pub match_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct MatchPlayerArgs {
    /// The match id, e.g. from `player_overview`.
    pub match_id: u64,
    /// The account id (Steam32 id) of a player in that match.
    pub account_id: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct LookupArgs {
    /// Part of a hero, ability or item name (e.g. "arctic", "kelvin"), or an id from match data.
    pub query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiSearchArgs {
    /// What to look for, e.g. "soul urn" or "Unstable Rift".
    pub query: String,
    /// How many results (default 5, at most 20).
    pub limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WikiPageArgs {
    /// The exact page title, from `wiki_search`.
    pub title: String,
    /// One section of the page (its heading); the answer lists the sections.
    pub section: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReviewMatchArgs {
    /// The player: a Steam name, an account id, a `SteamID64` or a profile link.
    pub player: String,
    /// The match to review; the player's latest match when left out.
    pub match_id: Option<String>,
}

/// Stateless: the routers are built by the macros, the caches live in `http` and `domain::assets`.
#[derive(Clone)]
pub struct DeadlockServer;

#[tool_router]
impl DeadlockServer {
    /// Find a player's account id by Steam name, `SteamID64` or profile link.
    #[tool(annotations(title = "Find a player", read_only_hint = true, open_world_hint = true))]
    async fn player_search(
        &self,
        Parameters(args): Parameters<PlayerSearchArgs>,
    ) -> Result<Json<player::Search>, CallToolResult> {
        run(move || player::search(&args.query)).await
    }

    /// A player's rank, win rate over counted matches, most played heroes and recent matches.
    #[tool(annotations(
        title = "Player overview",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn player_overview(
        &self,
        Parameters(args): Parameters<PlayerOverviewArgs>,
    ) -> Result<Json<player::Overview>, CallToolResult> {
        let recent = usize::try_from(args.recent.unwrap_or(10).min(50)).unwrap_or(10);
        run(move || player::overview(args.account_id, recent)).await
    }

    /// A match's mode, length and winner, and every player's hero, lane, K/D/A, souls, damage and own result.
    #[tool(annotations(title = "Match summary", read_only_hint = true, open_world_hint = true))]
    async fn match_summary(
        &self,
        Parameters(args): Parameters<MatchArgs>,
    ) -> Result<Json<matches::Summary>, CallToolResult> {
        run(move || matches::summary(args.match_id)).await
    }

    /// One player's match in depth: result and why, souls by source, unspent souls, deaths with killers and
    /// positions, purchases and permanent buffs.
    #[tool(annotations(
        title = "Player in a match",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn match_player(
        &self,
        Parameters(args): Parameters<MatchPlayerArgs>,
    ) -> Result<Json<matches::PlayerReport>, CallToolResult> {
        run(move || matches::player(args.match_id, args.account_id)).await
    }

    /// The three lanes of a match: who laned against whom and the soul lead every 3 minutes up to 15:00.
    #[tool(annotations(title = "Match lanes", read_only_hint = true, open_world_hint = true))]
    async fn match_lanes(
        &self,
        Parameters(args): Parameters<MatchArgs>,
    ) -> Result<Json<matches::Lanes>, CallToolResult> {
        run(move || matches::lanes(args.match_id)).await
    }

    /// Heroes (with their abilities), abilities and shop items by name or id.
    #[tool(annotations(
        title = "Look up a hero, ability or item",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn game_lookup(
        &self,
        Parameters(args): Parameters<LookupArgs>,
    ) -> Result<Json<game::Lookup>, CallToolResult> {
        run(move || game::lookup(&args.query)).await
    }

    /// Search the Deadlock Wiki for game mechanics, heroes and items.
    #[tool(annotations(
        title = "Search the Deadlock Wiki",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn wiki_search(
        &self,
        Parameters(args): Parameters<WikiSearchArgs>,
    ) -> Result<Json<wiki::Search>, CallToolResult> {
        run(move || wiki::search(&args.query, args.limit.unwrap_or(5))).await
    }

    /// The text of a Deadlock Wiki page, or of one of its sections.
    #[tool(annotations(
        title = "Read a Deadlock Wiki page",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn wiki_page(
        &self,
        Parameters(args): Parameters<WikiPageArgs>,
    ) -> Result<Json<wiki::Page>, CallToolResult> {
        run(move || wiki::page(&args.title, args.section.as_deref())).await
    }
}

#[prompt_router]
impl DeadlockServer {
    /// Review a player's match: result, lane, where the souls came from, deaths and purchases.
    #[prompt(name = "review_match")]
    async fn review_match(
        &self,
        Parameters(args): Parameters<ReviewMatchArgs>,
    ) -> Vec<PromptMessage> {
        let which = args.match_id.map_or_else(
            || "their latest match (the first one player_overview lists)".to_owned(),
            |id| format!("match {id}"),
        );
        let text = format!(
            "Review {which} for the Deadlock player {player:?}.\n\
             1. Resolve the player with player_search if needed, then get the match id from player_overview.\n\
             2. Call match_player and match_lanes; call match_summary for the whole lobby.\n\
             3. Explain the result (and why it wasn't scored, if so), how the lane went, where the souls came from, \
             how many souls sat unspent compared with the lobby, where and to whom they died, and what they bought when.\n\
             4. End with two or three concrete things to do differently. Check mechanics on the wiki before relying on them.",
            player = args.player,
        );
        vec![PromptMessage::new_text(Role::User, text)]
    }
}

#[derive(Serialize)]
struct HeroResource {
    id: u32,
    name: String,
    abilities: Vec<String>,
}

#[derive(Serialize)]
struct ItemResource<'a> {
    id: u32,
    name: &'a str,
    kind: &'a str,
    hero: Option<String>,
    slot: Option<&'a str>,
    tier: Option<u64>,
    cost: Option<u64>,
}

fn resource_text(uri: &str) -> Result<String, ErrorData> {
    let cat = catalog().map_err(|p| ErrorData::internal_error(p.message(), None))?;
    let json = match uri {
        "deadlock://heroes" => {
            let mut heroes: Vec<HeroResource> = cat
                .heroes
                .iter()
                .map(|(&id, name)| {
                    let mut abilities: Vec<String> = cat
                        .items
                        .values()
                        .filter(|i| i.hero == Some(id) && i.kind == "ability")
                        .map(|i| i.name.clone())
                        .collect();
                    abilities.sort();
                    HeroResource {
                        id,
                        name: name.clone(),
                        abilities,
                    }
                })
                .collect();
            heroes.sort_by(|a, b| a.name.cmp(&b.name));
            serde_json::to_string(&heroes)
        }
        "deadlock://items" => {
            let mut items: Vec<ItemResource<'_>> = cat
                .items
                .values()
                .filter(|i| i.kind == "upgrade")
                .map(|i| ItemResource {
                    id: i.id,
                    name: &i.name,
                    kind: &i.kind,
                    hero: i.hero.map(|h| cat.hero(u64::from(h))),
                    slot: i.slot.as_deref(),
                    tier: i.tier,
                    cost: i.cost,
                })
                .collect();
            items.sort_by(|a, b| (a.slot, a.tier, a.name).cmp(&(b.slot, b.tier, b.name)));
            serde_json::to_string(&items)
        }
        _ => {
            return Err(ErrorData::resource_not_found(
                format!("no resource {uri}"),
                None,
            ));
        }
    };
    json.map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for DeadlockServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
        // Not `Implementation::from_build_env()`: that reads rmcp's own crate name and version.
        .with_server_info(
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                .with_title("Deadlock"),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new("deadlock://heroes", "heroes")
                .with_description("Every hero with its id and abilities (JSON)")
                .with_mime_type("application/json"),
            Resource::new("deadlock://items", "items")
                .with_description("Every shop item with id, slot, tier and cost (JSON)")
                .with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let text = tokio::task::spawn_blocking(move || resource_text(&uri))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))??;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri)]).into())
    }
}
