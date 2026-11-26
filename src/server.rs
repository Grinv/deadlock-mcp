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

use crate::domain::assets::{ItemKind, catalog};
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

const REVIEW_STEPS: &str = "\
1. Resolve the player with player_search if needed, then get the match id from player_overview.
2. Call match_player and match_lanes; call match_summary for the whole lobby.
3. Explain the result (and why it wasn't scored, if so), how the lane went, where the souls came from, \
how many souls sat unspent compared with the lobby, where and to whom they died, and what they bought when.
4. End with two or three concrete things to do differently. Check mechanics on the wiki before relying on them.";

fn fail(problem: &Problem) -> CallToolResult {
    let mut result = CallToolResult::error(vec![ContentBlock::text(problem.message())]);
    result.structured_content = serde_json::to_value(problem).ok();
    result
}

/// The schema the model sees: derived from the argument type, which also carries the `garde` rules
/// that `parse` enforces, so the advertised limits and the checked ones can't drift apart.
fn input_schema<T: JsonSchema + std::any::Any>() -> std::sync::Arc<rmcp::model::JsonObject> {
    rmcp::handler::server::common::schema_for_input::<T>()
        .unwrap_or_else(|_| rmcp::handler::server::common::schema_for_type::<T>())
}

/// Arguments are taken raw and parsed here, so a bad value comes back as our `invalid-argument`
/// problem, naming the field.
fn parse<T>(raw: rmcp::model::JsonObject) -> Result<T, CallToolResult>
where
    T: serde::de::DeserializeOwned + garde::Validate<Context = ()>,
{
    let args: T =
        serde_path_to_error::deserialize(serde_json::Value::Object(raw)).map_err(|e| {
            let path = e.path().to_string();
            fail(&Problem::invalid_argument(format!(
                "{path}: {}",
                e.into_inner()
            )))
        })?;
    args.validate().map_err(|report| {
        let detail: Vec<String> = report
            .iter()
            .map(|(path, error)| format!("{path}: {error}"))
            .collect();
        fail(&Problem::invalid_argument(detail.join("; ")))
    })?;
    Ok(args)
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
            Kind::Internal,
            format!("the tool crashed: {e}"),
        ))),
    }
}

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

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct LookupArgs {
    /// Part of a hero, ability or item name (e.g. "arctic", "kelvin"), or an id from match data.
    #[garde(length(chars, min = 1, max = 100))]
    pub query: String,
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct WikiSearchArgs {
    /// What to look for, e.g. "soul urn" or "Unstable Rift".
    #[garde(length(chars, min = 1, max = 200))]
    pub query: String,
    /// How many results.
    #[serde(default = "default_limit")]
    #[garde(range(min = 1, max = 20))]
    pub limit: u32,
}

const fn default_limit() -> u32 {
    5
}

#[derive(Deserialize, JsonSchema, garde::Validate)]
#[serde(deny_unknown_fields)]
pub struct WikiPageArgs {
    /// The exact page title, from `wiki_search`.
    #[garde(length(chars, min = 1, max = 255))]
    pub title: String,
    /// One section of the page (its heading); the answer lists the sections.
    #[garde(inner(length(chars, min = 1, max = 200)))]
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

    /// One player's match in depth: result and why, souls by source, unspent souls, deaths with killers and
    /// positions, purchases and permanent buffs.
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

    /// Heroes (with their abilities), abilities and shop items by name or id.
    #[tool(
        title = "Look up a hero, ability or item",
        input_schema = input_schema::<LookupArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn game_lookup(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<game::Lookup>, CallToolResult> {
        let args: LookupArgs = parse(raw)?;
        run(move || game::lookup(&args.query)).await
    }

    /// Search the Deadlock Wiki for game mechanics, heroes and items.
    #[tool(
        title = "Search the Deadlock Wiki",
        input_schema = input_schema::<WikiSearchArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn wiki_search(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<wiki::Search>, CallToolResult> {
        let args: WikiSearchArgs = parse(raw)?;
        run(move || wiki::search(&args.query, args.limit)).await
    }

    /// The text of a Deadlock Wiki page, or of one of its sections.
    #[tool(
        title = "Read a Deadlock Wiki page",
        input_schema = input_schema::<WikiPageArgs>(),
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn wiki_page(
        &self,
        Parameters(raw): Parameters<rmcp::model::JsonObject>,
    ) -> Result<Json<wiki::Page>, CallToolResult> {
        let args: WikiPageArgs = parse(raw)?;
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
            "Review {which} for the Deadlock player {player:?}.\n{REVIEW_STEPS}",
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
    kind: ItemKind,
    hero: Option<String>,
    slot: Option<&'a str>,
    tier: Option<i32>,
    cost: Option<i64>,
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
                        .filter(|i| i.hero == Some(id) && i.kind == ItemKind::Ability)
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
                .filter(|i| i.kind == ItemKind::Upgrade)
                .map(|i| ItemResource {
                    id: i.id,
                    name: &i.name,
                    kind: i.kind,
                    hero: i.hero.map(|h| cat.hero(h)),
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

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's ServerHandler methods are async; these answer without awaiting"
)]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Tool names the instructions and the prompt tell the model to call must exist, and every
    /// tool must be introduced in the instructions (`match_*` covers the match tools).
    #[test]
    fn texts_name_real_tools() {
        let tools: Vec<String> = DeadlockServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect();
        let mentioned = |text: &str| -> Vec<String> {
            text.split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
                .filter(|w| {
                    ["player_", "match_", "game_", "wiki_"]
                        .iter()
                        .any(|p| w.starts_with(p) && w.len() > p.len())
                })
                .map(str::to_owned)
                .collect()
        };
        for name in mentioned(INSTRUCTIONS)
            .iter()
            .chain(&mentioned(REVIEW_STEPS))
        {
            assert!(tools.contains(name), "{name} is not a tool");
        }
        for tool in &tools {
            assert!(
                INSTRUCTIONS.contains(tool.as_str())
                    || (tool.starts_with("match_") && INSTRUCTIONS.contains("match_*")),
                "{tool} is missing from INSTRUCTIONS"
            );
        }
    }
}
