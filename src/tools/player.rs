//! `player_search`, `player_overview`, and a player's stats from the Deadlock API: `player_heroes`,
//! `player_mates`, `player_enemies`, `player_deaths_map` and `ingest_status`.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api::{
    EnemyStats, HeroStats, KillDeathStats, PlayerMatchHistoryEntry, RankResponse, SteamProfile,
};
use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::{date, mmss, plain};
use crate::domain::outcome::{PlayerResult, player_result};
use crate::http::{Source, optional_part};
use crate::problem::{Problem, Result};
use crate::sources::api;
use crate::tools::{Filters, answer_sources, hero_filter, id32, per, round, take, win_rate};

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
    if let Some(account_id) = account_id_of(query)? {
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
    let (found, source) = api::steam_search(q)?;
    search_from(query, found, source)
}

/// The first 10 accounts a Steam name search found.
fn search_from(query: &str, found: Vec<SteamProfile>, source: Source) -> Result<Search> {
    let players: Vec<Found> = found
        .into_iter()
        .filter_map(|p: SteamProfile| {
            Some(Found {
                account_id: u32::try_from(p.account_id).ok()?,
                steam_name: Some(plain(&p.personaname)),
                profile_url: Some(p.profileurl),
            })
        })
        .take(10)
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

/// The account an account id, a `SteamID64` or a steamcommunity.com/profiles/ link names; `None`
/// for a Steam name to search.
fn account_id_of(query: &str) -> Result<Option<u32>> {
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
        return Ok(Some(account_id));
    }
    if q.contains("steamcommunity.com/id/") {
        return Err(Problem::invalid_argument(
            "custom profile URLs (/id/...) can't be resolved without a Steam key; search by Steam name or give the SteamID64",
        ));
    }
    Ok(None)
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
    /// The rank badge at the end of the player's latest ranked match the Deadlock API holds, e.g.
    /// "Seeker III"; the meta tools' `min_rank` and `max_rank` take it as it is. `None` when the API
    /// holds no recent ranked match of the player: their rank is unknown, which is not the same as
    /// unranked, since the API only has the matches someone sent it.
    pub rank: Option<String>,
    /// Every match of the player's Steam match history.
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

pub fn overview(account_id: u32, recent: u32) -> Result<Overview> {
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
    let sources = answer_sources(
        [history_source, rank_source]
            .into_iter()
            .chain(profile_source),
        &cat,
    );
    Ok(overview_from(
        &cat, account_id, recent, &history, &rank, steam_name, sources,
    ))
}

fn overview_from(
    cat: &Catalog,
    account_id: u32,
    recent: u32,
    history: &[PlayerMatchHistoryEntry],
    rank: &RankResponse,
    steam_name: Option<String>,
    sources: Vec<Source>,
) -> Overview {
    let result_of = |m: &PlayerMatchHistoryEntry| {
        player_result(
            m.player_match_outcome,
            i64::from(m.player_team) == m.match_result,
        )
    };
    let hero_of = |m: &PlayerMatchHistoryEntry| cat.hero(id32(m.hero_id.into()));
    let (mut wins, mut losses, mut not_scored) = (0, 0, 0);
    let mut heroes: HashMap<i32, (u32, u32, u32)> = HashMap::new();
    for m in history {
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
    Overview {
        account_id,
        steam_name,
        rank: rank.last_match.as_ref().map(|_| cat.rank(rank.badge)),
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
            .take(take(recent))
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
        sources,
    }
}

/// `ids` sorted, each once.
fn distinct(ids: &[u32]) -> Vec<u32> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Steam names of `ids` (each asked once) in one request. Names only decorate an answer, so a
/// failure goes into the answer's sources and the answer goes on without them.
pub fn steam_names(ids: &[u32]) -> (HashMap<u32, String>, Option<Source>) {
    if ids.is_empty() {
        return (HashMap::new(), None);
    }
    let fetched = api::steam_profiles(&distinct(ids)).map(|(profiles, source)| {
        let names = profiles
            .into_iter()
            .filter_map(|p| Some((u32::try_from(p.account_id).ok()?, plain(&p.personaname))))
            .collect();
        (names, source)
    });
    let (names, source) = optional_part("steam names", fetched);
    (names, Some(source))
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
    /// Matches over all heroes in the time range, counting only the matches the Deadlock API has
    /// the full data of, so fewer than `player_overview`'s history.
    pub matches: i64,
    /// Heroes played, most played first.
    pub heroes: Vec<PlayerHero>,
    /// Heroes played but not listed (over `limit`).
    pub more_heroes: usize,
    pub sources: Vec<Source>,
}

pub fn heroes(account_id: u32, days: Option<u32>, limit: u32) -> Result<PlayerHeroes> {
    let cat = catalog()?;
    let (stats, source) = api::player_hero_stats(account_id, &days_query(days))?;
    if stats.is_empty() {
        return Err(Problem::not_found(format!(
            "the Deadlock API has no matches for account {account_id} in that time; check the id with player_search"
        )));
    }
    let sources = answer_sources([source], &cat);
    Ok(heroes_from(&cat, account_id, limit, stats, sources))
}

fn heroes_from(
    cat: &Catalog,
    account_id: u32,
    limit: u32,
    mut stats: Vec<HeroStats>,
    sources: Vec<Source>,
) -> PlayerHeroes {
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
    PlayerHeroes {
        account_id,
        matches,
        heroes,
        more_heroes,
        sources,
    }
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

/// The time range of a player's stats as query parameters.
fn days_query(days: Option<u32>) -> String {
    Filters {
        days,
        ..Filters::default()
    }
    .query()
}

fn player_filters(days: Option<u32>, min_matches: u32) -> String {
    format!("{}&min_matches_played={min_matches}", days_query(days))
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
    let (met, lost) = met_and_lost(&rows, limit);
    let ids: Vec<u32> = met.iter().chain(&lost).map(|e| id32(e.enemy_id)).collect();
    let (names, names_source) = steam_names(&ids);
    Ok(Enemies {
        account_id,
        most_met: partners(met.iter().map(row), &names),
        most_lost_to: partners(lost.iter().map(row), &names),
        sources: std::iter::once(source).chain(names_source).collect(),
    })
}

/// The opponents met most often and those lost to most often, at most `limit` of each.
fn met_and_lost(rows: &[EnemyStats], limit: u32) -> (Vec<&EnemyStats>, Vec<&EnemyStats>) {
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
    (met, lost)
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
    /// The player's deaths and kills in the time range, in the matches the Deadlock API has the
    /// full data of.
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
    let (hero_query, hero) = hero_filter(&cat, hero)?;
    let query = format!("&account_ids={account_id}{}{hero_query}", days_query(days));
    let (cells, source) = api::kill_death_stats(&query)?;
    let sources = answer_sources([source], &cat);
    Ok(deaths_map_from(
        &cat, account_id, hero, limit, &cells, sources,
    ))
}

fn deaths_map_from(
    cat: &Catalog,
    account_id: u32,
    hero: Option<String>,
    limit: u32,
    cells: &[KillDeathStats],
    sources: Vec<Source>,
) -> DeathsMap {
    let mut squares: HashMap<(i32, i32), (i64, i64)> = HashMap::new();
    for c in cells {
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
    DeathsMap {
        account_id,
        hero,
        deaths: cells.iter().map(|c| c.deaths).sum(),
        kills: cells.iter().map(|c| c.kills).sum(),
        death_hotspots: spots(true),
        kill_hotspots: spots(false),
        sources,
    }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn name_search_lists_ten_accounts() {
        let profile = |id: i64, name: &str| -> SteamProfile {
            serde_json::from_value(json!({
                "account_id": id, "avatar": "", "avatarfull": "", "avatarmedium": "",
                "countrycode": null, "friends": [], "last_team_avg_badge": null,
                "last_updated": "", "matches_played_last_30d": 0, "personaname": name,
                "profileurl": format!("https://steamcommunity.com/profiles/{id}/"), "realname": null,
            }))
            .unwrap()
        };
        let source = || Source::failed(&Problem::not_found("x"));
        // An id outside u32 is dropped; hidden characters leave the name; at most 10 stay.
        let mut found = vec![profile(-1, "bad"), profile(7, "Mi\u{200b}ka")];
        found.extend((100..112).map(|id| profile(id, "same")));
        let s = search_from("mika", found, source()).unwrap();
        let ids: Vec<u32> = s.players.iter().map(|p| p.account_id).collect();
        assert_eq!(ids, [7, 100, 101, 102, 103, 104, 105, 106, 107, 108]);
        assert_eq!(s.players[0].steam_name.as_deref(), Some("Mika"));
        assert_eq!(
            s.players[0].profile_url.as_deref(),
            Some("https://steamcommunity.com/profiles/7/")
        );
        assert_eq!(
            search_from("nobody", Vec::new(), source())
                .err()
                .map(|e| e.message()),
            Some("Not found: no Steam account named like \"nobody\" in the Deadlock API; try another spelling or the SteamID64".to_owned())
        );
    }

    #[test]
    fn search_reads_ids_and_profile_links() {
        let id = |q: &str| account_id_of(q).map_err(|p| p.message());
        assert_eq!(id("12345"), Ok(Some(12345)));
        assert_eq!(id(" 76561197960266728 "), Ok(Some(1000)));
        assert_eq!(id("76561197960265728"), Ok(Some(0)));
        assert_eq!(
            id("https://steamcommunity.com/profiles/76561197960266728/"),
            Ok(Some(1000))
        );
        assert_eq!(id("steamcommunity.com/profiles/1000?l=en"), Ok(Some(1000)));
        // Names, even with digits in them, are searched.
        assert_eq!(id("Haze"), Ok(None));
        assert_eq!(id("123abc"), Ok(None));
        assert_eq!(id("https://steamcommunity.com/profiles/"), Ok(None));
        assert_eq!(
            id(" 99999999999"),
            Err(
                "Invalid argument: \" 99999999999\" is neither an account id nor a SteamID64"
                    .to_owned()
            )
        );
        assert_eq!(
            id("https://steamcommunity.com/id/someone/"),
            Err("Invalid argument: custom profile URLs (/id/...) can't be resolved without a Steam key; search by Steam name or give the SteamID64".to_owned())
        );
    }

    #[test]
    fn overview_counts_wins_losses_and_unscored() {
        let mut history: Vec<PlayerMatchHistoryEntry> =
            serde_json::from_str(include_str!("../../contracts/fixtures/match_history.json"))
                .unwrap();
        // The fixture's matches: a loss on hero 63, a win on 72, a loss on 63; make the last one
        // unscored.
        history[2].player_match_outcome = 5;
        let rank: RankResponse =
            serde_json::from_str(include_str!("../../contracts/fixtures/rank.json")).unwrap();
        let cat = Catalog::test(&[(63, "H63"), (72, "H72")]);
        let o = overview_from(&cat, 1, 2, &history, &rank, None, Vec::new());
        assert_eq!(
            serde_json::to_value(&o).unwrap(),
            json!({
                "account_id": 1,
                "steam_name": null,
                "rank": "badge 36",
                "matches": 3,
                "wins": 1,
                "losses": 1,
                "not_scored": 1,
                "win_rate": 0.5,
                "top_heroes": [
                    {"hero": "H63", "matches": 2, "wins": 0, "losses": 1},
                    {"hero": "H72", "matches": 1, "wins": 1, "losses": 0},
                ],
                "recent": [
                    {
                        "match_id": 143_785_700,
                        "date": "2026-11-26",
                        "hero": "H63",
                        "result": "loss",
                        "kills": 7,
                        "deaths": 12,
                        "assists": 16,
                        "net_worth": 55787,
                        "duration": "49:29",
                    },
                    {
                        "match_id": 143_768_397,
                        "date": "2026-11-26",
                        "hero": "H72",
                        "result": "win",
                        "kills": 16,
                        "deaths": 8,
                        "assists": 22,
                        "net_worth": 62191,
                        "duration": "39:53",
                    },
                ],
                "sources": [],
            })
        );
    }

    /// Without a ranked match in the API the badge is 0, which would read as "Obscurus".
    #[test]
    fn overview_rank_is_unknown_without_a_ranked_match() {
        let history: Vec<PlayerMatchHistoryEntry> =
            serde_json::from_str(include_str!("../../contracts/fixtures/match_history.json"))
                .unwrap();
        let mut rank: RankResponse =
            serde_json::from_str(include_str!("../../contracts/fixtures/rank.json")).unwrap();
        (rank.badge, rank.last_match) = (0, None);
        let cat = Catalog::test(&[(63, "H63"), (72, "H72")]);
        let o = overview_from(&cat, 1, 2, &history, &rank, None, Vec::new());
        assert_eq!(o.rank, None);
    }

    #[test]
    fn steam_names_ask_each_account_once() {
        assert_eq!(distinct(&[7, 3, 7, 1, 3]), [1, 3, 7]);
    }

    #[test]
    fn heroes_most_played_first() {
        let stats: Vec<HeroStats> = serde_json::from_str(include_str!(
            "../../contracts/fixtures/player_hero_stats.json"
        ))
        .unwrap();
        let h = heroes_from(
            &Catalog::test(&[(1, "H1"), (6, "H6")]),
            1,
            1,
            stats,
            Vec::new(),
        );
        let heroes: Vec<_> = h
            .heroes
            .iter()
            .map(|h| (h.hero.as_str(), h.matches))
            .collect();
        assert_eq!((h.matches, heroes, h.more_heroes), (3, vec![("H6", 2)], 1));
    }

    #[test]
    fn enemies_by_matches_and_by_losses() {
        let rows: Vec<EnemyStats> = [
            (1, 3, 4),
            (2, 0, 2),
            (3, 2, 4),
            (4, 5, 5),
            (5, 1, 3),
            (6, 2, 4),
        ]
        .into_iter()
        .map(|(enemy_id, wins, matches_played)| EnemyStats {
            enemy_id,
            matches: Vec::new(),
            matches_played,
            wins,
        })
        .collect();
        let (met, lost) = met_and_lost(&rows, 5);
        let ids = |v: Vec<&EnemyStats>| v.iter().map(|e| e.enemy_id).collect::<Vec<_>>();
        // Most matches first; ties by id; the sixth (2 matches) is cut.
        assert_eq!(ids(met), [4, 1, 3, 6, 5]);
        // Most losses first, then the lowest win rate (0, 1/3, 2/4, 2/4), then id; never lost to 4.
        assert_eq!(ids(lost), [2, 5, 3, 6, 1]);
    }

    #[test]
    fn deaths_map_sums_squares() {
        let mut cells: Vec<KillDeathStats> = serde_json::from_str(include_str!(
            "../../contracts/fixtures/kill_death_stats.json"
        ))
        .unwrap();
        let cell = |position_x, position_y, deaths| KillDeathStats {
            deaths,
            killer_team: 0,
            kills: 0,
            position_x,
            position_y,
        };
        // In the square of the fixture's (-768, -3840), and one more death further east.
        cells.push(cell(-700, -3500, 2));
        cells.push(cell(5000, -100, 1));
        let m = deaths_map_from(
            &Catalog::test(&[]),
            1,
            Some("Infernus".to_owned()),
            2,
            &cells,
            Vec::new(),
        );
        let spots = |v: &[Hotspot]| {
            v.iter()
                .map(|s| (s.x, s.y, s.deaths, s.kills))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            (m.hero.as_deref(), m.deaths, m.kills),
            (Some("Infernus"), 5, 1)
        );
        // Centers of 1024-unit squares, rounded down also below zero; ties by position.
        assert_eq!(
            spots(&m.death_hotspots),
            [(-512, -3584, 3, 0), (2560, 4608, 1, 0)]
        );
        assert_eq!(spots(&m.kill_hotspots), [(-4608, -1536, 0, 1)]);
    }
}
