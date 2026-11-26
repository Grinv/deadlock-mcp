//! `player_search` and `player_overview`.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::domain::assets::catalog;
use crate::domain::format::{date, mmss};
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
        let (profile, _) = api::steam_profiles(&[account_id])?;
        let p = profile.as_array().and_then(|a| a.first());
        return Ok(Search {
            players: vec![Found {
                account_id,
                steam_name: p.and_then(|p| p["personaname"].as_str()).map(str::to_owned),
                profile_url: p.and_then(|p| p["profileurl"].as_str()).map(str::to_owned),
            }],
        });
    }
    if q.contains("steamcommunity.com/id/") {
        return Err(Problem::invalid_argument(
            "custom profile URLs (/id/...) can't be resolved without a Steam key; search by Steam name or give the SteamID64",
        ));
    }
    let (found, _) = api::steam_search(q)?;
    let players: Vec<Found> = found
        .as_array()
        .into_iter()
        .flatten()
        .take(10)
        .filter_map(|p| {
            Some(Found {
                account_id: u32::try_from(p["account_id"].as_u64()?).ok()?,
                steam_name: p["personaname"].as_str().map(str::to_owned),
                profile_url: p["profileurl"].as_str().map(str::to_owned),
            })
        })
        .collect();
    if players.is_empty() {
        return Err(Problem::not_found(format!(
            "no Steam account named like {query:?} in the Deadlock API; try another spelling or the SteamID64"
        )));
    }
    Ok(Search { players })
}

#[derive(Serialize, JsonSchema)]
pub struct RecentMatch {
    pub match_id: u64,
    pub date: String,
    pub hero: String,
    pub result: PlayerResult,
    pub kills: u64,
    pub deaths: u64,
    pub assists: u64,
    pub net_worth: u64,
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
    let Some(history) = history.as_array().filter(|h| !h.is_empty()) else {
        return Err(Problem::not_found(format!(
            "the Deadlock API has no matches for account {account_id}; check the id with player_search"
        )));
    };
    let (rank, rank_source) = api::rank(account_id)?;
    let badge = rank["badge"].as_u64().unwrap_or(0);
    let profile = api::steam_profiles(&[account_id]).ok();
    let steam_name = profile.as_ref().and_then(|(p, _)| {
        p.as_array()?.first()?["personaname"]
            .as_str()
            .map(str::to_owned)
    });

    let result_of = |m: &Value| {
        player_result(
            m["player_match_outcome"].as_u64().unwrap_or(0),
            m["player_team"].as_u64().unwrap_or(0),
            m["match_result"].as_u64().unwrap_or(u64::MAX),
        )
    };
    let (mut wins, mut losses, mut not_scored) = (0, 0, 0);
    let mut heroes: HashMap<u64, (u32, u32, u32)> = HashMap::new();
    for m in history {
        let r = result_of(m);
        let entry = heroes
            .entry(m["hero_id"].as_u64().unwrap_or(0))
            .or_default();
        entry.0 += 1;
        match r {
            PlayerResult::Win => (wins, entry.1) = (wins + 1, entry.1 + 1),
            PlayerResult::Loss => (losses, entry.2) = (losses + 1, entry.2 + 1),
            PlayerResult::NotScored => not_scored += 1,
        }
    }
    let mut top: Vec<(u64, (u32, u32, u32))> = heroes.into_iter().collect();
    top.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let scored = wins + losses;
    Ok(Overview {
        account_id,
        steam_name,
        rank: cat.rank(badge),
        matches: u32::try_from(history.len()).unwrap_or(u32::MAX),
        wins,
        losses,
        not_scored,
        win_rate: (scored > 0).then(|| f64::from(wins) / f64::from(scored)),
        top_heroes: top
            .into_iter()
            .take(5)
            .map(|(h, (m, w, l))| HeroRecord {
                hero: cat.hero(h),
                matches: m,
                wins: w,
                losses: l,
            })
            .collect(),
        recent: history
            .iter()
            .take(recent)
            .map(|m| RecentMatch {
                match_id: m["match_id"].as_u64().unwrap_or(0),
                date: date(m["start_time"].as_i64().unwrap_or(0)),
                hero: cat.hero(m["hero_id"].as_u64().unwrap_or(0)),
                result: result_of(m),
                kills: m["player_kills"].as_u64().unwrap_or(0),
                deaths: m["player_deaths"].as_u64().unwrap_or(0),
                assists: m["player_assists"].as_u64().unwrap_or(0),
                net_worth: m["net_worth"].as_u64().unwrap_or(0),
                duration: mmss(m["match_duration_s"].as_u64().unwrap_or(0)),
            })
            .collect(),
        sources: vec![history_source, rank_source],
    })
}
