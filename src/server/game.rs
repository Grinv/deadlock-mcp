//! The game-knowledge tools: lookup and the wiki.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{DeadlockServer, input_schema, parse, run};
use crate::tools::game;

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

#[tool_router(router = game_tools, vis = "pub(super)")]
impl DeadlockServer {
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
    ) -> Result<Json<game::Search>, CallToolResult> {
        let args: WikiSearchArgs = parse(raw)?;
        run(move || game::search(&args.query, args.limit)).await
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
    ) -> Result<Json<game::Page>, CallToolResult> {
        let args: WikiPageArgs = parse(raw)?;
        run(move || game::page(&args.title, args.section.as_deref())).await
    }
}
