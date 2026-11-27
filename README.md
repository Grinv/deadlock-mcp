# Deadlock MCP

An [MCP](https://modelcontextprotocol.io) server for Deadlock, Valve's hero shooter. It gives an AI
assistant match and player statistics from the [Deadlock API](https://deadlock-api.com) and game
knowledge from the [Deadlock Wiki](https://deadlock.wiki). It only reads public data over the
network and never touches game files or needs a Steam login.

Ask things like:

- "My Steam name is Kestrel. Review my last match: why did we lose the lane, and what should I
  have bought instead?"
- "Who left that match, and when?"
- "Which items win on Kelvin at Oracle rank, and which work against Haze?"
- "Who do I win most with, which heroes beat me, and where on the map do I keep dying?"
- "Who counters Vindicta, and what should I skill first on her?"
- "What changed in the last patch?" or "Tell me Paige's backstory."

## Tools

Each tool answers one question; the assistant combines them.

| Area | Tools |
|---|---|
| Players | `player_search` (account id from a Steam name, SteamID64 or profile link), `player_overview`, `player_heroes`, `player_mates`, `player_enemies`, `player_deaths_map`, `ingest_status` |
| Matches | `match_summary`, `match_player` (souls by source, unspent souls, deaths with killers and the nearest objective, purchases, award stats such as crates broken and damage by type), `match_lanes` |
| Meta | `hero_stats`, `hero_items`, `hero_counters`, `hero_synergies`, `ability_order`, `performance_curve` (the average net worth, K/D/A and souls by source over a match), `stat_percentiles` (where a match's stats rank), `builds`, `leaderboard`, `patches` |
| Game | `game_lookup` (heroes, abilities and items with their tooltips), `wiki_search`, `wiki_page` |

Meta tools take a rank range by name (`min_rank`, `max_rank`, e.g. "Ascendant" or "Eternus III")
and a period (`days`). Every win rate comes with the number of matches behind it, and small samples are left out.

A player's `result` is their own. When the game voids a match for a player, usually because a
teammate left, it is `not_scored`; a leaver's match is a loss even if their team won.

The server also has a prompt, `review_match`, that walks through a player's match and compares it
with the meta. Its resources are `deadlock://heroes`, `deadlock://items` and one
`deadlock://hero/{id}` per hero (role, lore, abilities); matches are read through the template
`deadlock://match/{match_id}`. Clients can complete a hero id from its name.

## What it can't do

- A match is there only once the Deadlock API has it. The API gets a match when one of its players
  runs the API's ingest tool or when the API's bots pick it up, so a match from a minute ago, or an
  old one nobody sent, may be missing. `ingest_status` tells whether a player's matches arrive.
- Unranked matches carry no ranks: Valve sends a player's rank only in ranked matches, so for an
  unranked lobby the server can't say how strong the players were.
- The meta covers the matches the Deadlock API holds, not every game played.
- It can't run your own queries over the whole match database, follow a match while it is being
  played, or see anything that needs a Steam login. It never changes anything.
- The wiki is written by players and can lag behind a patch; item and ability tooltips from
  `game_lookup` come from the game files and follow the patch.

## deadlock-mcp and the Deadlock API's own MCP server

The Deadlock API runs an MCP server of its own at `https://api.deadlock-api.com/v1/mcp`: read-only
SQL (DuckDB) over hourly snapshots of its database, plus lists of heroes, items, patches and ranked
seasons. It can answer questions nobody wrote a tool for. deadlock-mcp answers the common ones
directly, with the data already read for the assistant:

- A player is found by Steam name, SteamID64 or profile link (`player_search`). When several
  accounts share a name, up to ten are listed for the assistant to choose from.
- Answers use the game's terms. Ranks go in and come out by name ("Ascendant", "Eternus III").
  Lanes and the objective nearest each death are named from the game files. An award says whether
  it was earned and at which threshold.
- A player's result is the one the game recorded for them, not their team's: when a teammate leaves,
  the match doesn't count for the others, and a leaver loses even when the team wins.
- Numbers come with what they mean. Every win rate has its match count and small samples are left
  out; a counter's win rate says whose it is; a new hero's pick rate counts only the days since its
  release.
- A question is usually one call. "Who counters Haze at Ascendant?" is one `hero_counters` call; a
  match review is `match_player`, `match_summary` and `match_lanes`, with the steps in the
  `review_match` prompt. With SQL the assistant first has to learn the schema and its pitfalls, such
  as voided matches and the rows that appear twice.
- Answers hold only what the question needs: names rather than internal class names, plain text
  rather than HTML, no image links. `game_lookup` for Haze is 1,620 characters with the abilities by
  name; the Deadlock API server's details of Haze add 17 image links, a 3D model, a 36-row level
  table and the abilities as class names (`ability_sleep_dagger`).
- Mechanics and lore come from the Deadlock Wiki, and item and ability tooltips keep their values.
- A failure shows in the answer: a stale or missing source is marked, and a wrong argument comes back
  with what was expected (an unknown rank lists the ranks).

| | deadlock-mcp | The Deadlock API's MCP server |
|---|---|---|
| How the assistant asks | One tool per question, with checked arguments | It writes SQL against the database schema |
| A player's match | Ready answers: own result, souls by source, unspent souls, deaths with killers and places, awards, lanes | Possible with SQL, if the assistant knows the columns and pitfalls |
| Meta and custom statistics | Common questions: hero and item win rates, counters, synergies, ability orders, stat percentiles | Any aggregate over all matches |
| Freshness | The REST API's current answers | Snapshots refreshed hourly |
| Limits | The REST API's rate limits | `execute_query`: 30 calls a minute, 400 a day, 1,024 rows and 50 KB per result |
| Game knowledge | Wiki pages and lore, tooltips with values, patch notes | Hero, item, patch and ranked-season lists, patch notes |
| Runs | Locally, over stdio | Hosted, over HTTP |

## Download and use

Prebuilt releases are planned: an `.mcpb` bundle for one-click install in Claude Desktop, and
installers for Linux, macOS and Windows. Until then, build it with Rust 1.100 or newer:

```sh
cargo install --locked --git https://github.com/Grinv/deadlock-mcp deadlock-mcp
```

Then add it to your MCP client. Claude Code:

```sh
claude mcp add deadlock -- deadlock-mcp
```

Clients configured with JSON (Claude Desktop and others):

```json
{
  "mcpServers": {
    "deadlock": { "command": "deadlock-mcp" }
  }
}
```

Start by telling the assistant who you are on Steam (your name, profile link or SteamID64); it finds
your account and recent matches from there. To review a match step by step, use the `review_match`
prompt. Nothing needs a key or a login.

## Data, caching and limits

- Match and player data, the game catalog and the meta come from the Deadlock API, a community
  project not affiliated with Valve. Mechanics and lore come from the Deadlock Wiki. Every answer
  lists its `sources`.
- Answers are cached on disk in `$XDG_CACHE_HOME/deadlock-mcp` (`~/.cache/deadlock-mcp`, or
  `%LOCALAPPDATA%\deadlock-mcp` on Windows). Finished matches stay cached until unused; other
  answers expire after 60 seconds to 24 hours and are revalidated with `ETag`. When a source fails,
  an older copy is used and marked `stale`; when an optional part (Steam names, place names, awards)
  can't be fetched at all, the answer leaves it out and its source carries the `error`. Entries unused for 30 days are deleted when the server
  starts. To clear the cache, delete the folder.
- The catalog of heroes, items, ranks and lanes follows the newest game version the Deadlock API
  has processed, so after a patch the names update once the API catches up.
- The server sends at most 10 requests per second to each site. It retries once after a network
  error, a `5xx` or a `Retry-After` of up to 10 seconds; a longer `Retry-After` comes back as a
  `rate-limited` error with the wait time.
- A failing tool returns an [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) problem; the types
  are described in [docs/errors.md](docs/errors.md).

## Privacy

The server has no telemetry and sends nothing to its maintainer. [PRIVACY.md](PRIVACY.md) lists
what each request carries and what the cache keeps.

## Licenses

The code is under the [MIT license](LICENSE). Wiki text is
[CC BY-NC-SA 4.0](https://creativecommons.org/licenses/by-nc-sa/4.0/), and the wiki tools say so in
their answers. Valve's protobuf definitions in `contracts/protos` come from the game files through
[SteamDatabase/Protobufs](https://github.com/SteamDatabase/Protobufs) (see
[contracts/SOURCES.md](contracts/SOURCES.md)). Deadlock is a trademark of Valve Corporation, and
this project is not affiliated with Valve.

## Development

[AGENTS.md](AGENTS.md) has the layout, the commands and the rules for changes.
