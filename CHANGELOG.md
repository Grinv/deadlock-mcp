# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
