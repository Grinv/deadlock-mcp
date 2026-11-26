//! The MCP surface: tools, prompts and resources. Each handler parses its arguments, runs the
//! blocking tool logic off the async runtime and turns a `Problem` into an error result.

mod game;
mod matches;
mod meta;
mod player;
mod resources;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    CallToolResult, CompleteRequestParams, CompleteResult, CompletionInfo, ContentBlock,
    Implementation, ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    PromptMessage, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Reference,
    Resource, ResourceContents, ResourceTemplate, Role, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, prompt, prompt_handler, prompt_router, tool_handler,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::problem::{Kind, Problem};
use resources::{HERO_TEMPLATE, MATCH_TEMPLATE, hero_completions, hero_resources, resource_text};

const INSTRUCTIONS: &str = "\
Deadlock (Valve's hero shooter) statistics and game knowledge.
- Players are identified by account id (the Steam32 id). Start with player_search when you only have a name, a SteamID64 or a profile link.
- player_overview lists a player's recent matches with their match ids; the match_* tools take those ids.
- A player's `result` is their own: `not_scored` when the game voided the match for them (usually a teammate left); a leaver's match is a loss even if their team won.
- Every answer lists its `sources`; when one is `stale`, the data source failed and an older copy was used, so say so.
- Hero, ability and item names come from game_lookup; mechanics and explanations from wiki_search and wiki_page (cite the page URL; the text is CC BY-NC-SA 4.0).
- A hero's story is on the wiki page \"<Hero>/Lore\" (e.g. \"Kelvin/Lore\"); the hero page itself only links to it.
- A player's own stats: player_heroes (per hero), player_mates (who they play with), player_enemies (who they meet and lose to), \
player_deaths_map (where on the map they die and kill; positions as in match_player, with the objective they are near). ingest_status says whether the player \
runs the API's ingest tool; without it the API may miss some of their matches, so totals can be low.
- The meta over all players: hero_stats (win and pick rate per hero), and for one hero hero_items (shop items), hero_counters \
(enemies), hero_synergies (teammates) and ability_order. performance_curve gives the average net worth, K/D/A and \
souls by source at each stats snapshot of a match; stat_percentiles where a match's per-match stats (souls per minute, \
K/D/A, damage, accuracy...) rank; its percentiles rank single matches, not players. Filter them to a player's rank with min_badge/max_badge (the badge \
behind player_overview's rank) and say how many matches a rate rests on; small samples mislead.
- builds lists in-game builds of a hero by popularity; leaderboard the top players of a region; patches the patch notes \
(pass a patch's link for its text) to tell whether a stat predates a balance change.
- game_lookup gives ability and item tooltips: description, what each ability upgrade adds, cooldowns and other values.
- Resources: hero cards at deadlock://hero/<hero id> (role, lore, abilities with tooltips) and match summaries at \
deadlock://match/<match id>.";

const REVIEW_STEPS: &str = "\
1. Resolve the player with player_search if needed, then get the match id from player_overview.
2. Call match_player and match_lanes; call match_summary for the whole lobby.
3. Explain the result (and why it wasn't scored, if so), how the lane went, where the souls came from, \
how many souls sat unspent compared with the lobby, where and to whom they died, and what they bought when.
4. Compare with the meta at the player's rank (min_badge/max_badge around their badge): hero_items for their hero, \
and against the enemy heroes of this match (enemy_heroes); ability_order for their skill order; hero_counters \
for their lane opponent. Mention the match counts behind the rates.
5. Compare their net worth over time (match_player) with performance_curve for their hero and rank, \
and their souls per minute, K/D/A and damage with stat_percentiles for the same hero and rank.
6. If they died often, player_deaths_map with their hero shows whether those places are a habit.
7. End with two or three concrete things to do differently. Check mechanics on the wiki before relying on them.";

pub(super) fn fail(problem: &Problem) -> CallToolResult {
    let mut result = CallToolResult::error(vec![ContentBlock::text(problem.message())]);
    result.structured_content = serde_json::to_value(problem).ok();
    result
}

/// The schema the model sees: derived from the argument type, which also carries the `garde` rules
/// that `parse` enforces, so the advertised limits and the checked ones can't drift apart.
pub(super) fn input_schema<T: JsonSchema + std::any::Any>()
-> std::sync::Arc<rmcp::model::JsonObject> {
    rmcp::handler::server::common::schema_for_input::<T>()
        .unwrap_or_else(|_| rmcp::handler::server::common::schema_for_type::<T>())
}

/// Arguments are taken raw and parsed here, so a bad value comes back as our `invalid-argument`
/// problem, naming the field.
pub(super) fn parse<T>(raw: rmcp::model::JsonObject) -> Result<T, CallToolResult>
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
pub(super) async fn run<T, F>(f: F) -> Result<Json<T>, CallToolResult>
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

pub(super) const fn default_ten() -> u32 {
    10
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

impl DeadlockServer {
    /// All tools, merged from the per-area routers.
    fn tools() -> ToolRouter<Self> {
        Self::player_tools() + Self::match_tools() + Self::meta_tools() + Self::game_tools()
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

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ErrorData> + Send + 'static,
) -> Result<T, ErrorData> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's ServerHandler methods are async; these answer without awaiting"
)]
#[tool_handler(router = Self::tools())]
#[prompt_handler]
impl ServerHandler for DeadlockServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .enable_completions()
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
        let heroes = blocking(hero_resources).await?;
        Ok(ListResourcesResult::with_all_items(
            [
                Resource::new("deadlock://heroes", "heroes")
                    .with_description("Every hero with its id and abilities (JSON)")
                    .with_mime_type("application/json"),
                Resource::new("deadlock://items", "items")
                    .with_description("Every shop item with id, slot, tier and cost (JSON)")
                    .with_mime_type("application/json"),
            ]
            .into_iter()
            .chain(heroes)
            .collect(),
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new(HERO_TEMPLATE, "hero")
                .with_title("Hero card")
                .with_description(
                    "A hero's role, playstyle, lore and abilities with their tooltips (JSON). \
                     Hero ids are in deadlock://heroes or from game_lookup.",
                )
                .with_mime_type("application/json"),
            ResourceTemplate::new(MATCH_TEMPLATE, "match")
                .with_title("Match summary")
                .with_description("The match_summary answer for a match id (JSON)")
                .with_mime_type("application/json"),
        ]))
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        // Only the hero id is cheap to complete; player names would need a Steam search per keystroke.
        let values = match &request.r#ref {
            Reference::Resource(r) if r.uri == HERO_TEMPLATE && request.argument.name == "id" => {
                let typed = request.argument.value;
                blocking(move || hero_completions(&typed)).await?
            }
            _ => Vec::new(),
        };
        let info = CompletionInfo::new(values).map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(CompleteResult::new(info))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let text = blocking(move || resource_text(&uri)).await?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, request.uri).with_mime_type("application/json"),
        ])
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The README's tool table lists every tool.
    #[test]
    fn readme_lists_every_tool() {
        let readme = include_str!("../../README.md");
        for tool in DeadlockServer::tools().list_all() {
            assert!(
                readme.contains(&format!("`{}`", tool.name)),
                "{} is missing from README.md",
                tool.name
            );
        }
    }

    /// Tool names the instructions and the prompt tell the model to call must exist, and every
    /// tool must be introduced in the instructions (`match_*` covers the match tools).
    #[test]
    fn texts_name_real_tools() {
        let tools: Vec<String> = DeadlockServer::tools()
            .list_all()
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect();
        let mentioned = |text: &str| -> Vec<String> {
            text.split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
                .filter(|w| {
                    [
                        "player_",
                        "match_",
                        "game_",
                        "wiki_",
                        "hero_",
                        "ability_",
                        "ingest_",
                        "performance_",
                        "stat_",
                    ]
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
