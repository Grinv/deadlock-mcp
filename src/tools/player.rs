//! `player_search`, `player_overview`, and a player's stats from the Deadlock API: `player_heroes`,
//! `player_mates`, `player_enemies`, `player_deaths_map` and `ingest_status`.

use std::collections::HashMap;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api::{PlayerMatchHistoryEntry, SteamProfile};
use crate::domain::assets::catalog;
use crate::domain::format::{date, mmss, plain};
use crate::domain::outcome::{PlayerResult, player_result};
use crate::http::Source;
use crate::problem::{Problem, Result};
use crate::sources::api;
use crate::tools::{Filters, id32, per, round, take, win_rate};

const STEAM64_BASE: u64 = 76_561_197_960_265_728;

#[derive(Serialize, JsonSchema)]
pub struct Found {
    pub account_id: u32,
    pub steam_name: Option<String>,
    pub profile_url: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Search {
    pub players: Vec<Found>,
    pub sources: Vec<Source>,
}

/// An account id, a `SteamID64` or a steamcommunity.com/profiles/ link resolves directly; anything
/// else is searched as a Steam name.
pub fn search(query: &str) -> Result<Search> {
    let q = query.trim();
    let digits: String = q
        .split("/profiles/")
        .nth(1)
        .unwrap_or(q)
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if let Ok(n) = digits.parse::<u64>()
        && (digits.len() == q.len() || q.contains("/profiles/"))
    {
        let id = if n >= STEAM64_BASE {
            n - STEAM64_BASE
        } else {
            n
        };
        let Ok(account_id) = u32::try_from(id) else {
            return Err(Problem::invalid_argument(format!(
                "{query:?} is neither an account id nor a SteamID64"
            )));
        };
        let (profiles, source) = api::steam_profiles(&[account_id])?;
        let p = profiles.into_iter().next();
        return Ok(Search {
            players: vec![Found {
                account_id,
                steam_name: p.as_ref().map(|p| plain(&p.personaname)),
                profile_url: p.map(|p| p.profileurl),
            }],
            sources: vec![source],
        });
    }
    if q.contains("steamcommunity.com/id/") {
        return Err(Problem::invalid_argument(
            "custom profile URLs (/id/...) can't be resolved without a Steam key; search by Steam name or give the SteamID64",
        ));
    }
    let (found, source) = api::steam_search(q)?;
    let players: Vec<Found> = found
        .into_iter()
        .take(10)
        .filter_map(|p: SteamProfile| {
            Some(Found {
                account_id: u32::try_from(p.account_id).ok()?,
                steam_name: Some(plain(&p.personaname)),
                profile_url: Some(p.profileurl),
            })
        })
        .collect();
    if players.is_empty() {
        return Err(Problem::not_found(format!(
            "no Steam account named like {query:?} in the Deadlock API; try another spelling or the SteamID64"
        )));
    }
    Ok(Search {
        players,
        sources: vec![source],
    })
}

#[derive(Serialize, JsonSchema)]
pub struct RecentMatch {
    pub match_id: i64,
    pub date: String,
    pub hero: String,
    pub result: PlayerResult,
    pub kills: i64,
    pub deaths: i64,
    pub assists: i64,
    pub net_worth: i64,
    pub duration: String,
}

#[derive(Serialize, JsonSchema)]
pub struct HeroRecord {
    pub hero: String,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
}

#[derive(Serialize, JsonSchema)]
pub struct Overview {
    pub account_id: u32,
    pub steam_name: Option<String>,
    /// The rank badge, e.g. "Seeker III"; "Obscurus" when unranked.
    pub rank: String,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    /// Matches the game didn't count for this player; left out of the win rate.
    pub not_scored: u32,
    /// Wins over counted matches (wins + losses), 0..1.
    pub win_rate: Option<f64>,
    pub top_heroes: Vec<HeroRecord>,
    pub recent: Vec<RecentMatch>,
    pub sources: Vec<Source>,
}

pub fn overview(account_id: u32, recent: usize) -> Result<Overview> {
    let cat = catalog()?;
    let (history, history_source) = api::match_history(account_id)?;
    if history.is_empty() {
        return Err(Problem::not_found(format!(
            "the Deadlock API has no matches for account {account_id}; check the id with player_search"
        )));
    }
    let (rank, rank_source) = api::rank(account_id)?;
    let (mut names, profile_source) = steam_names(&[account_id]);
    let steam_name = names.remove(&account_id);

    let result_of = |m: &PlayerMatchHistoryEntry| {
        player_result(
            m.player_match_outcome,
            i64::from(m.player_team) == m.match_result,
        )
    };
    let hero_of = |m: &PlayerMatchHistoryEntry| cat.hero(id32(m.hero_id.into()));
    let (mut wins, mut losses, mut not_scored) = (0, 0, 0);
    let mut heroes: HashMap<i32, (u32, u32, u32)> = HashMap::new();
    for m in &history {
        let entry = heroes.entry(m.hero_id).or_default();
        entry.0 += 1;
        match result_of(m) {
            PlayerResult::Win => (wins, entry.1) = (wins + 1, entry.1 + 1),
            PlayerResult::Loss => (losses, entry.2) = (losses + 1, entry.2 + 1),
            PlayerResult::NotScored => not_scored += 1,
        }
    }
    let mut top: Vec<(i32, (u32, u32, u32))> = heroes.into_iter().collect();
    top.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let scored = wins + losses;
    Ok(Overview {
        account_id,
        steam_name,
        rank: cat.rank(rank.badge),
        matches: u32::try_from(history.len()).unwrap_or(u32::MAX),
        wins,
        losses,
        not_scored,
        win_rate: win_rate(i64::from(wins), i64::from(scored)),
        top_heroes: top
            .into_iter()
            .take(5)
            .map(|(h, (m, w, l))| HeroRecord {
                hero: cat.hero(id32(h.into())),
                matches: m,
                wins: w,
                losses: l,
            })
            .collect(),
        recent: history
            .iter()
            .take(recent)
            .map(|m| RecentMatch {
                match_id: m.match_id,
                date: date(m.start_time),
                hero: hero_of(m),
                result: result_of(m),
                kills: m.player_kills,
                deaths: m.player_deaths,
                assists: m.player_assists,
                net_worth: m.net_worth,
                duration: mmss(u64::try_from(m.match_duration_s).unwrap_or_default()),
            })
            .collect(),
        sources: [history_source, rank_source]
            .into_iter()
            .chain(profile_source)
            .chain(cat.degraded_sources())
            .collect(),
    })
}

/// Steam names of `ids` in one request. Names only decorate an answer, so a failure goes into the
/// answer's sources and the answer goes on without them.
pub fn steam_names(ids: &[u32]) -> (HashMap<u32, String>, Option<Source>) {
    if ids.is_empty() {
        return (HashMap::new(), None);
    }
    match api::steam_profiles(ids) {
        Ok((profiles, source)) => (
            profiles
                .into_iter()
                .filter_map(|p| Some((u32::try_from(p.account_id).ok()?, plain(&p.personaname))))
                .collect(),
            Some(source),
        ),
        Err(problem) => {
            eprintln!("deadlock-mcp: steam names: {}", problem.message());
            (HashMap::new(), Some(Source::failed(&problem)))
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub struct PlayerHero {
    pub hero: String,
    pub matches: i64,
    pub wins: i64,
    /// Wins over matches, 0..1.
    pub win_rate: Option<f64>,
    pub kills_per_match: f64,
    pub deaths_per_match: f64,
    pub assists_per_match: f64,
    pub souls_per_min: f64,
    pub player_damage_per_min: f64,
    /// Shots hit over shots fired, 0..1.
    pub accuracy: f64,
    pub hours_played: f64,
    pub last_played: String,
}

#[derive(Serialize, JsonSchema)]
pub struct PlayerHeroes {
    pub account_id: u32,
    /// Matches over all heroes in the time range.
    pub matches: i64,
    /// Heroes played, most played first.
    pub heroes: Vec<PlayerHero>,
    /// Heroes played but not listed (over `limit`).
    pub more_heroes: usize,
    pub sources: Vec<Source>,
}

pub fn heroes(account_id: u32, days: Option<u32>, limit: u32) -> Result<PlayerHeroes> {
    let cat = catalog()?;
    let filters = Filters {
        days,
        ..Filters::default()
    };
    let (mut stats, source) = api::player_hero_stats(account_id, &filters.query())?;
    if stats.is_empty() {
        return Err(Problem::not_found(format!(
            "the Deadlock API has no matches for account {account_id} in that time; check the id with player_search"
        )));
    }
    stats.sort_by(|a, b| {
        b.matches_played
            .cmp(&a.matches_played)
            .then(a.hero_id.cmp(&b.hero_id))
    });
    let matches = stats.iter().map(|h| h.matches_played).sum();
    let more_heroes = stats.len().saturating_sub(take(limit));
    let heroes = stats
        .iter()
        .take(take(limit))
        .map(|h| PlayerHero {
            hero: cat.hero(id32(h.hero_id.into())),
            matches: h.matches_played,
            wins: h.wins,
            win_rate: win_rate(h.wins, h.matches_played),
            kills_per_match: per(h.kills, h.matches_played),
            deaths_per_match: per(h.deaths, h.matches_played),
            assists_per_match: per(h.assists, h.matches_played),
            souls_per_min: round(h.networth_per_min, 1),
            player_damage_per_min: round(h.damage_per_min, 1),
            accuracy: round(h.accuracy, 3),
            hours_played: per(h.time_played, 3600),
            last_played: date(h.last_played),
        })
        .collect();
    Ok(PlayerHeroes {
        account_id,
        matches,
        heroes,
        more_heroes,
        sources: std::iter::once(source)
            .chain(cat.degraded_sources())
            .collect(),
    })
}

/// Another player and how matches with or against them went.
#[derive(Serialize, JsonSchema)]
pub struct Partner {
    pub account_id: u32,
    pub steam_name: Option<String>,
    pub matches: i64,
    /// Matches the player won with (or against) them.
    pub wins: i64,
    /// Wins over matches, 0..1.
    pub win_rate: Option<f64>,
}

fn partners(
    rows: impl Iterator<Item = (u32, i64, i64)>,
    names: &HashMap<u32, String>,
) -> Vec<Partner> {
    rows.map(|(id, matches, wins)| Partner {
        account_id: id,
        steam_name: names.get(&id).cloned(),
        matches,
        wins,
        win_rate: win_rate(wins, matches),
    })
    .collect()
}

fn player_filters(days: Option<u32>, min_matches: u32) -> String {
    let filters = Filters {
        days,
        ..Filters::default()
    };
    format!("{}&min_matches_played={min_matches}", filters.query())
}

#[derive(Serialize, JsonSchema)]
pub struct PlayerMates {
    pub account_id: u32,
    /// Only mates who queued in the same party.
    pub same_party: bool,
    /// Most matches together first.
    pub mates: Vec<Partner>,
    pub sources: Vec<Source>,
}

pub fn mates(
    account_id: u32,
    same_party: bool,
    days: Option<u32>,
    min_matches: u32,
    limit: u32,
) -> Result<PlayerMates> {
    let query = format!(
        "{}&same_party={same_party}",
        player_filters(days, min_matches)
    );
    let (mut rows, source) = api::mate_stats(account_id, &query)?;
    rows.sort_by(|a, b| {
        b.matches_played
            .cmp(&a.matches_played)
            .then(b.wins.cmp(&a.wins))
    });
    rows.truncate(take(limit));
    let ids: Vec<u32> = rows.iter().map(|m| id32(m.mate_id)).collect();
    let (names, names_source) = steam_names(&ids);
    Ok(PlayerMates {
        account_id,
        same_party,
        mates: partners(
            rows.iter()
                .map(|m| (id32(m.mate_id), m.matches_played, m.wins)),
            &names,
        ),
        sources: std::iter::once(source).chain(names_source).collect(),
    })
}

#[derive(Serialize, JsonSchema)]
pub struct Enemies {
    pub account_id: u32,
    /// Opponents met most often; `wins` are the player's wins against them.
    pub most_met: Vec<Partner>,
    /// Opponents the player lost to most often (most losses, then the lowest win rate).
    pub most_lost_to: Vec<Partner>,
    pub sources: Vec<Source>,
}

pub fn enemies(
    account_id: u32,
    days: Option<u32>,
    min_matches: u32,
    limit: u32,
) -> Result<Enemies> {
    let (rows, source) = api::enemy_stats(account_id, &player_filters(days, min_matches))?;
    let row = |e: &&crate::contracts::api::EnemyStats| (id32(e.enemy_id), e.matches_played, e.wins);
    let mut met: Vec<_> = rows.iter().collect();
    met.sort_by(|a, b| {
        b.matches_played
            .cmp(&a.matches_played)
            .then(a.enemy_id.cmp(&b.enemy_id))
    });
    met.truncate(take(limit));
    let mut lost: Vec<_> = rows.iter().filter(|e| e.matches_played > e.wins).collect();
    lost.sort_by(|a, b| {
        let losses = |e: &crate::contracts::api::EnemyStats| e.matches_played - e.wins;
        losses(b)
            .cmp(&losses(a))
            .then((a.wins * b.matches_played).cmp(&(b.wins * a.matches_played)))
            .then(a.enemy_id.cmp(&b.enemy_id))
    });
    lost.truncate(take(limit));
    let mut ids: Vec<u32> = met.iter().chain(&lost).map(|e| id32(e.enemy_id)).collect();
    ids.sort_unstable();
    ids.dedup();
    let (names, names_source) = steam_names(&ids);
    Ok(Enemies {
        account_id,
        most_met: partners(met.iter().map(row), &names),
        most_lost_to: partners(lost.iter().map(row), &names),
        sources: std::iter::once(source).chain(names_source).collect(),
    })
}

/// The API's raster cells are 128 map units; hotspots sum them into squares of this size.
const HOTSPOT: i32 = 1024;

#[derive(Serialize, JsonSchema)]
pub struct Hotspot {
    /// Center of a 1024-unit square, in the map coordinates `match_player` gives death positions in.
    pub x: i32,
    pub y: i32,
    /// The objective within about 50 m of the center, if any.
    pub near: Option<String>,
    pub deaths: i64,
    pub kills: i64,
}

#[derive(Serialize, JsonSchema)]
pub struct DeathsMap {
    pub account_id: u32,
    pub hero: Option<String>,
    /// The player's deaths and kills in the time range.
    pub deaths: i64,
    pub kills: i64,
    /// Where the player died most, most deaths first.
    pub death_hotspots: Vec<Hotspot>,
    /// Where the player got the most kills, most kills first.
    pub kill_hotspots: Vec<Hotspot>,
    pub sources: Vec<Source>,
}

pub fn deaths_map(
    account_id: u32,
    hero: Option<&str>,
    days: Option<u32>,
    limit: u32,
) -> Result<DeathsMap> {
    let cat = catalog()?;
    let hero_id = hero.map(|h| cat.find_hero(h)).transpose()?;
    let filters = Filters {
        days,
        ..Filters::default()
    };
    let mut query = format!("&account_ids={account_id}{}", filters.query());
    if let Some(h) = hero_id {
        let _ = write!(query, "&hero_ids={h}");
    }
    let (cells, source) = api::kill_death_stats(&query)?;
    let mut squares: HashMap<(i32, i32), (i64, i64)> = HashMap::new();
    for c in &cells {
        let sq = squares
            .entry((
                c.position_x.div_euclid(HOTSPOT),
                c.position_y.div_euclid(HOTSPOT),
            ))
            .or_default();
        sq.0 += c.deaths;
        sq.1 += c.kills;
    }
    let spots = |by_deaths: bool| -> Vec<Hotspot> {
        let mut v: Vec<Hotspot> = squares
            .iter()
            .map(|(&(x, y), &(deaths, kills))| {
                let (x, y) = (x * HOTSPOT + HOTSPOT / 2, y * HOTSPOT + HOTSPOT / 2);
                Hotspot {
                    x,
                    y,
                    near: cat.near(f64::from(x), f64::from(y)),
                    deaths,
                    kills,
                }
            })
            .filter(|s| if by_deaths { s.deaths > 0 } else { s.kills > 0 })
            .collect();
        v.sort_by_key(|s| {
            let n = if by_deaths { s.deaths } else { s.kills };
            (std::cmp::Reverse(n), s.x, s.y)
        });
        v.truncate(take(limit));
        v
    };
    Ok(DeathsMap {
        account_id,
        hero: hero_id.map(|h| cat.hero(h)),
        deaths: cells.iter().map(|c| c.deaths).sum(),
        kills: cells.iter().map(|c| c.kills).sum(),
        death_hotspots: spots(true),
        kill_hotspots: spots(false),
        sources: std::iter::once(source)
            .chain(cat.degraded_sources())
            .collect(),
    })
}

#[derive(Serialize, JsonSchema)]
pub struct IngestStatus {
    pub account_id: u32,
    /// The player ran the deadlock-api-ingest tool within the window, so the API gets their
    /// matches (with replays' details) as they play them.
    pub active: bool,
    /// Date of the latest verified upload in the window.
    pub last_upload: Option<String>,
    pub window_days: i64,
    pub sources: Vec<Source>,
}

pub fn ingest_status(account_id: u32) -> Result<IngestStatus> {
    let (s, source) = api::ingest_status(account_id)?;
    Ok(IngestStatus {
        account_id,
        active: s.active,
        last_upload: s.last_upload_at.map(date),
        window_days: s.window_days,
        sources: vec![source],
    })
}
