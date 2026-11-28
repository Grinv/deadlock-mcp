# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-11-28

### Added

- `match_damage`: who damaged a player and whom they damaged, by ability, weapon and item, who
  finished them (killing damage) and the seconds of crowd control they took, up to a minute or the
  match end. The non-heroes (troopers, objectives, the Midboss, summons) are reported apart.
- `match_distance`: how many seconds a player stood within 10, 20, 30, 45 and 60 m of each other
  hero in a window of the match.
- `lane_matchups`: how a lane duo does against an enemy duo or against any duo: the average soul
  lead with its spread and the damage lead at each stats snapshot, and the win rate. The API marks
  the data behind it as subject to change.
- `item_timings`: which shop items win on a hero in each phase of a match (0-9, 9-20, 20-30 and
  30+ minutes), with the win rate adjusted for the buyer's net worth at purchase next to the raw
  one, and each phase's win rate and matches.
- `match_summary` lists the objectives destroyed and the Midboss kills (who killed it, who claimed
  the Rejuvenator, and when), and says whether the game matched the lobby in its new player or low
  priority pool.
- `match_player` gives the player's ability upgrades in order, their kill participation, and the
  build they had selected when the match started, when the API has analysed the match's replay.
- `builds` gives each build's matches and win rate over the last 30 days; with `build_id` it gives
  one build and its items in the author's groups.
- `hero_counters` and `hero_synergies` take `heroes`: each named hero's win rate, matches and place
  among all pairs with its direction (`hardest_place`, `best_place`), next to the hero's average.
- `hero_stats` takes `min_hero_matches` and `max_hero_matches` to count only players with that many
  matches on the hero, e.g. up to 5 for players new to it.

### Changed

- `hero_items` now warns that its win rates are raw and favour items bought by players already
  ahead; `item_timings` adjusts for that.
- The `review_match` prompt starts from the whole match, compares the lane with both opponents, the
  partner and the usual result of that matchup, and checks each conclusion against a second figure.
- The built-in server instructions are shorter.
- The repository's copy of the Deadlock API's OpenAPI document now carries the API's MIT license.

### Fixed

- Seven heroes were missing an ability, among them Solomon's Develop Queen and Graves' Borrowed
  Decree. All their abilities are listed now.
- `player_overview` gives `rank: null` when the Deadlock API knows no ranked match of the player.
  It said "Obscurus" (unranked) before, though the rank is only unknown.
- An error page from the gateway in front of the API comes back as its title, e.g. "504: Gateway
  time-out", instead of its HTML. A 504 is no longer retried: the API can answer the same query
  with the same 504 for minutes, so the error says to narrow the query instead.

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
