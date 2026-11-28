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
    ResourceContents, Role, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, prompt, prompt_handler, prompt_router, tool_handler,
};
use schemars::JsonSchema;
use schemars::transform::{RestrictFormats, Transform as _};
use serde::Deserialize;

use crate::problem::{Kind, Problem};
use resources::{HERO_TEMPLATE, hero_completions, resource_text};

const INSTRUCTIONS: &str = "\
Deadlock (Valve's hero shooter) statistics and game knowledge.
- Players are identified by account id (the Steam32 id): player_search finds it from a name, a SteamID64 or a profile link. \
player_overview lists their recent matches; the match_* tools take those match ids.
- A player's `result` is their own: `not_scored` when the game voided the match for them (usually a teammate left); a \
leaver's match is a loss even if their team won.
- Every answer lists its `sources`. A `stale` one failed and an older copy was used: say so. One with an `error` failed \
with nothing cached, so what it gives (Steam names, place names, awards) is missing, not empty.
- A player's own stats: player_heroes, player_mates, player_enemies, player_deaths_map. The API has only the matches \
someone sent it; ingest_status says whether the player sends theirs, and if not, totals can be low.
- The meta over all players: hero_stats, hero_items, item_timings (items by match phase, adjusted for the buyer's \
lead), hero_counters, hero_synergies, lane_matchups (a lane duo against another), ability_order, builds, leaderboard; \
performance_curve and stat_percentiles place one match against the average. Filter them to the player's rank \
(min_rank/max_rank), say how many matches a rate rests on (small samples mislead), and check patches for a balance \
change since.
- game_lookup gives hero, ability and item names and tooltips; wiki_search and wiki_page the mechanics (cite the page \
URL; the text is CC BY-NC-SA 4.0). A hero's story is on the wiki page \"<Hero>/Lore\".
- Resources: hero cards at deadlock://hero/<hero id>, match summaries at deadlock://match/<match id>.";

const REVIEW_STEPS: &str = "\
1. Find the player with player_search if needed and take the match id from player_overview. Note how experienced \
they are (player_heroes: their matches on this hero; if few, hero_stats with max_hero_matches 5 shows how the hero \
does for players new to it) and whether the match is ranked (match_summary's mode).
2. Read the whole match first: match_summary (both teams' K/D/A, net worth and MVP, whether the game matched it in \
the new player or low priority pool, and the Midboss: while a team holds its Rejuvenator, it can lose a fight without \
deaths), match_lanes (each lane's soul lead at every snapshot), then match_player for the player.
3. In the player's lane, call match_player for both lane opponents and the lane partner and compare souls by source \
(lane, jungle, kills, objectives, urn), net worth over time, when each bought what, and the order of ability upgrades \
against ability_order. Compare the lane's soul lead from match_lanes (from the player's side) with lane_matchups for \
the player and partner against the two lane opponents: its mean and spread at the same snapshot. Call match_damage \
with until_minute 15 for who hurt the player and who finished them, with what, and match_distance with until_minute 15 \
for the time they spent near each lane opponent. Read that time against the abilities' ranges from game_lookup for the \
opponents' and the player's heroes.
4. Compare the player with the hero's meta, at their rank when the match is ranked (min_rank/max_rank as \
player_overview names it), over all ranks otherwise: performance_curve (their net worth at each snapshot against the \
average and its spread: when they fell behind, and which souls they lacked) and stat_percentiles (deaths, souls per \
minute, damage dealt and taken per minute, last hits, assists).
5. Matchups: hero_counters with heroes set to the lane opponents (same_lane true) and hero_synergies with heroes set \
to the lane partner. Read each pairing against average_win_rate and give its place with the direction (\"8th hardest \
of 43\"), not a bare number.
6. Deaths: who killed them, where and when (match_player); player_deaths_map with their hero if dying looks like a \
habit. Items: what they bought when, against item_timings for the hero at their rank (its adjusted win rates) and \
the build they selected (builds with build_id set to the build match_player gives, when known), next to what the \
enemies built and hero_items against this match's enemy heroes (enemy_heroes).
7. Before stating a conclusion, check it against a second figure that could contradict it: read assists against \
kill_participation, and take much damage taken as a sign they were in the fights. Keep data and inference apart and \
give the match counts behind rates. Don't call a choice a mistake until you know where it came from: their build, \
or ask.
8. End with two or three concrete things to do differently, each tied to a figure. Check mechanics on the wiki before \
relying on them.";

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

/// The window of the meta tools and the deaths map when `days` is left out. A typed default, so the
/// schema shows it and no tool can leave the window open by accident.
pub(super) const fn default_window_days() -> u32 {
    30
}

#[derive(Deserialize, JsonSchema)]
pub struct ReviewMatchArgs {
    /// The player: an account id, a `SteamID64`, a profile link or a Steam name. Prefer an id: a
    /// client that splits command arguments at spaces cuts a name with a space in it.
    pub player: String,
    /// The match to review; the player's latest match when left out.
    pub match_id: Option<String>,
}

/// Stateless: the routers are built by the macros, the caches live in `http` and `domain::assets`.
#[derive(Clone)]
pub struct DeadlockServer;

impl DeadlockServer {
    /// All tools, merged from the per-area routers, with schemas in standard JSON Schema.
    fn tools() -> ToolRouter<Self> {
        let mut router =
            Self::player_tools() + Self::match_tools() + Self::meta_tools() + Self::game_tools();
        for route in router.map.values_mut() {
            let tool = &mut route.attr;
            tool.input_schema = std::sync::Arc::new(standard_formats(&tool.input_schema));
            tool.output_schema = tool
                .output_schema
                .as_deref()
                .map(|s| std::sync::Arc::new(standard_formats(s)));
        }
        router
    }
}

/// `schema` without schemars' `uint` formats (`uint32`, `uint64`, `uint` for `usize`...): no
/// standard defines them, so clients can't use them, and the MCP TypeScript SDK's validator logs
/// each one when it compiles a result schema. The formats JSON Schema defines stay, and so do `OpenAPI`'s `int32`, `int64`,
/// `float` and `double`, which validators know; bounds are in `minimum`/`maximum`.
fn standard_formats(schema: &rmcp::model::JsonObject) -> rmcp::model::JsonObject {
    let mut restrict = RestrictFormats::default();
    restrict
        .allowed_formats
        .extend(["int32", "int64", "float", "double"].map(Into::into));
    let mut schema = schemars::Schema::from(schema.clone());
    restrict.transform(&mut schema);
    schema.as_object().cloned().unwrap_or_default()
}

#[prompt_router]
impl DeadlockServer {
    /// Review a player's match: the whole match, the lane against both opponents (souls, damage,
    /// distance), deaths, purchases, ability order and matchups.
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
        Ok(ListResourcesResult::with_all_items(
            blocking(resources::resources).await?,
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(
            resources::templates(),
        ))
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

    #[test]
    fn schemas_use_standard_formats() {
        let schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "id": {"type": "integer", "format": "uint32", "minimum": 0},
                "when": {"type": "string", "format": "date-time"}
            },
            "$defs": {"Row": {"anyOf": [{"type": "number", "format": "double"}, {"type": "null"}],
                              "properties": {"at": {"type": "integer", "format": "uint64"}}}},
            "examples": [{"format": "uint32"}]
        });
        let serde_json::Value::Object(schema) = schema else {
            unreachable!()
        };
        // The uint formats go; standard and OpenAPI formats and data inside the schema stay.
        assert_eq!(
            serde_json::Value::Object(standard_formats(&schema)),
            serde_json::json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "minimum": 0},
                    "when": {"type": "string", "format": "date-time"}
                },
                "$defs": {"Row": {"anyOf": [{"type": "number", "format": "double"}, {"type": "null"}],
                                  "properties": {"at": {"type": "integer"}}}},
                "examples": [{"format": "uint32"}]
            })
        );
        // No published schema keeps a uint format.
        let published = serde_json::to_string(&DeadlockServer::tools().list_all()).unwrap();
        assert!(!published.contains("\"uint"), "a uint format is left");
    }

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
                        "lane_",
                        "item_",
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
