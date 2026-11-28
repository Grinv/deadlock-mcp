# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `item_timings`: which shop items win on a hero in each phase of a match (0-9, 9-20, 20-30 and
  30+ minutes), by the API's win rate adjusted for the buyer's net worth at purchase, next to the
  raw one, each phase's win rate and how many matches reached it. `hero_items` says its win rates
  are raw and favour items bought by players already ahead.
- `lane_matchups`: how a lane duo does against an enemy duo, or against any duo: the average soul
  lead with its spread and the damage lead at each stats snapshot, and the win rate. Built on the
  API's lane soul curve and lane matchup stats, which the API marks as subject to change.
- `match_damage`: who damaged a player and whom they damaged, by ability, weapon and item, who
  finished them (killing damage) and the seconds of crowd control they took, up to a minute or the
  match end; the non-heroes (troopers, objectives, the Midboss, summons) are reported apart.
- `match_distance`: how many seconds a player stood within 10, 20, 30, 45 and 60 m of each other
  hero in a window of the match.
- `match_summary` lists the objectives destroyed, in order; `match_player` the ability upgrades, in
  order.
- `hero_counters` and `hero_synergies` take `heroes`: each named hero's win rate, matches and place
  among all pairs with its direction (`hardest_place`, `best_place`), next to the hero's average
  win rate.
- `match_player` gives the player's kill participation; `match_summary` says whether the game
  matched the lobby in its new player or low priority pool.

### Changed

- `contracts/openapi.json` carries the Deadlock API's MIT license in `contracts/openapi.LICENSE`.
- The server instructions are shorter: a map of the tools and the rules that span them.
- The `review_match` prompt starts from the whole match, compares the lane with both opponents and
  the partner, and checks each conclusion against a second figure.

### Fixed

- An error page from the gateway in front of the API (Cloudflare) comes back as its title, e.g.
  "504: Gateway time-out", instead of its HTML, and a 504 says to narrow the query rather than being retried:
  the API answers the same query with the same 504 for minutes.
- Seven heroes lost an ability whose tooltip has no text of its own, among them Solomon's Develop
  Queen and Graves' Borrowed Decree; a hero's abilities are now its signature and ultimate ones.
- `player_overview` gives `rank: null` when the API holds no ranked match of the player, instead of
  "Obscurus", which read as unranked.

## [0.1.0] - 2026-11-27

### Added

- `match_player` lists the stats behind the end-of-match awards, the earned ones first with the
  threshold reached, among them crates broken, damage by bullets, abilities and melee, and
  headshots.
- Players: find an account by Steam name, SteamID64 or profile link; a player's rank, win rate,
  heroes, teammates, opponents, where on the map they die and kill, and whether the API receives
  their matches.
- Matches: summary of all twelve players, one player's match in depth (souls by source, unspent
  souls, deaths with killers, purchases, permanent buffs) and the soul lead per lane.
- Meta by rank range (rank names as `player_overview` gives them, e.g. "Ascendant") and period:
  hero win and pick rates, items, counters, synergies, ability orders, in-game builds,
  leaderboards and patch notes. Win rates come with their match counts.
- Game knowledge: heroes, abilities and items with their tooltips, and Deadlock Wiki search and
  pages, lore included.
- A player's own result: a match the game voided for them is `not_scored`, and a leaver's match is
  a loss even when their team won.
- The `review_match` prompt, hero resources with name completion, and a match resource template.
- Answers list their sources and flag stale copies and parts that couldn't be fetched; errors are
  RFC 9457 problems.
- Answers are cached on disk; entries unused for 30 days are deleted at startup.
- An `.mcpb` bundle for Claude Desktop and other MCPB clients, with the server for macOS, Windows
  and Linux; an npm package (`npx -y deadlock-mcp`) that installs only the build for the machine;
  and an entry in the MCP Registry for both.
- The catalog follows the newest game version the Deadlock API has processed.
- Deaths in `match_player` and hotspots in `player_deaths_map` name the objective they are near,
  e.g. "The Archmother's York Guardian"; `match_player` gives the net worth at each stats snapshot.
- `performance_curve`: the average player's net worth, K/D/A, damage and souls by source over a
  match, and `stat_percentiles`: percentiles of per-match stats, both by hero, rank and period.
