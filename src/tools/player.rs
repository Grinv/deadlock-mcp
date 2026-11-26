//! `player_search` and `player_overview`.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api::{PlayerMatchHistoryEntry, SteamProfile};
use crate::domain::assets::catalog;
use crate::domain::format::{date, mmss, plain};
use crate::domain::outcome::{PlayerResult, player_result};
use crate::http::Source;
use crate::problem::{Problem, Result};
use crate::sources::api;

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
    // The name is a nicety: the overview stands without it, so log and go on.
    let (steam_name, profile_source) = match api::steam_profiles(&[account_id]) {
        Ok((profiles, source)) => (
            profiles.into_iter().next().map(|p| plain(&p.personaname)),
            Some(source),
        ),
        Err(problem) => {
            eprintln!("deadlock-mcp: steam name: {}", problem.message());
            (None, None)
        }
    };

    let result_of = |m: &PlayerMatchHistoryEntry| {
        player_result(
            m.player_match_outcome,
            i64::from(m.player_team) == m.match_result,
        )
    };
    let hero_of =
        |m: &PlayerMatchHistoryEntry| cat.hero(u32::try_from(m.hero_id).unwrap_or_default());
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
        win_rate: (scored > 0).then(|| f64::from(wins) / f64::from(scored)),
        top_heroes: top
            .into_iter()
            .take(5)
            .map(|(h, (m, w, l))| HeroRecord {
                hero: cat.hero(u32::try_from(h).unwrap_or_default()),
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
            .chain(cat.stale_sources())
            .collect(),
    })
}
