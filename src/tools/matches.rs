//! `match_summary`, `match_player` and `match_lanes`, from a match's metadata.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::valve::ECitadelLobbyTeam;
use crate::contracts::valve::c_msg_match_meta_data_contents::{
    MatchInfo, PlayerStats, Players, Position,
};
use crate::domain::assets::{Catalog, ItemKind, Lane, catalog};
use crate::domain::format::{date, mmss, plain};
use crate::domain::outcome::{PlayerResult, note, player_result};
use crate::domain::terms::{game_mode_name, gold_source_name, match_mode_name, team_name};
use crate::http::Source;
use crate::problem::{Kind, Problem, Result};
use crate::sources::api;

const TEAMS: [ECitadelLobbyTeam; 2] = [
    ECitadelLobbyTeam::KECitadelLobbyTeamTeam0,
    ECitadelLobbyTeam::KECitadelLobbyTeamTeam1,
];
/// Samples the per-player stats come in during the laning phase.
const LANE_SAMPLES_S: [u32; 5] = [180, 360, 540, 720, 900];

fn load(match_id: u64) -> Result<(MatchInfo, Source)> {
    let (meta, source) = api::match_metadata(match_id).map_err(|p| {
        if p.kind == Kind::NotFound {
            Problem::not_found(format!(
                "the Deadlock API doesn't have match {match_id}. It gets a match once one of its 12 players \
                 sends it with deadlock-api-ingest, or a bot watches it; check the id, or try again later"
            ))
            .with_instance(p.instance.unwrap_or_default())
        } else {
            p
        }
    })?;
    let info = meta.match_info.ok_or_else(|| {
        Problem::new(
            Kind::InvalidResponse,
            format!("match {match_id} has no match_info"),
        )
        .with_instance(source.url.clone())
    })?;
    Ok((info, source))
}

fn last_stat(p: &Players) -> Option<&PlayerStats> {
    p.stats.last()
}

fn result_of(info: &MatchInfo, p: &Players) -> PlayerResult {
    player_result(
        p.player_match_outcome.unwrap_or_default(),
        p.team() == info.winning_team(),
    )
}

fn left_at(p: &Players) -> Option<String> {
    Some(p.abandon_match_time_s())
        .filter(|&s| s > 0)
        .map(|s| mmss(u64::from(s)))
}

/// Steam names of the match's players; an empty map when the lookup fails.
fn steam_names(info: &MatchInfo) -> (HashMap<u32, String>, Option<Source>) {
    let ids: Vec<u32> = info.players.iter().map(Players::account_id).collect();
    match api::steam_profiles(&ids) {
        Ok((profiles, source)) => {
            let names = profiles
                .into_iter()
                .filter_map(|p| Some((u32::try_from(p.account_id).ok()?, plain(&p.personaname))))
                .collect();
            (names, Some(source))
        }
        // Names are a nicety: the answer stands without them, so log and go on.
        Err(problem) => {
            eprintln!("deadlock-mcp: steam names: {}", problem.message());
            (HashMap::new(), None)
        }
    }
}

fn find_player(info: &MatchInfo, match_id: u64, account_id: u32) -> Result<&Players> {
    info.players
        .iter()
        .find(|p| p.account_id() == account_id)
        .ok_or_else(|| {
            Problem::invalid_argument(format!(
                "account {account_id} didn't play match {match_id}; match_summary lists its players"
            ))
        })
}

fn mode(info: &MatchInfo) -> String {
    format!(
        "{} ({})",
        match_mode_name(info.match_mode()),
        game_mode_name(info.game_mode())
    )
}

// ---- match_summary ----

#[derive(Serialize, JsonSchema)]
pub struct SummaryPlayer {
    pub account_id: u32,
    pub steam_name: Option<String>,
    pub hero: String,
    pub lane: Lane,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub net_worth: u32,
    pub hero_damage: u32,
    /// This player's own result; differs from the team's when the game voided or penalized it.
    pub result: PlayerResult,
    /// Time the game flagged the player as having left (they may have reconnected).
    pub abandoned_at: Option<String>,
    pub mvp: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct SummaryTeam {
    pub team: &'static str,
    pub won: bool,
    pub players: Vec<SummaryPlayer>,
}

#[derive(Serialize, JsonSchema)]
pub struct Summary {
    pub match_id: u64,
    pub date: String,
    /// Match mode and game mode, e.g. "Ranked (Normal)".
    pub mode: String,
    pub duration: String,
    pub winner: &'static str,
    pub teams: Vec<SummaryTeam>,
    pub sources: Vec<Source>,
}

pub fn summary(match_id: u64) -> Result<Summary> {
    let cat = catalog()?;
    let (info, source) = load(match_id)?;
    let (names, names_source) = steam_names(&info);
    let winner = info.winning_team();
    let teams = TEAMS
        .into_iter()
        .map(|team| SummaryTeam {
            team: team_name(team),
            won: team == winner,
            players: info
                .players
                .iter()
                .filter(|p| p.team() == team)
                .map(|p| SummaryPlayer {
                    account_id: p.account_id(),
                    steam_name: names.get(&p.account_id()).cloned(),
                    hero: cat.hero(p.hero_id()),
                    lane: cat.lane(p.assigned_lane()),
                    kills: p.kills(),
                    deaths: p.deaths(),
                    assists: p.assists(),
                    net_worth: p.net_worth(),
                    hero_damage: last_stat(p).map_or(0, PlayerStats::player_damage),
                    result: result_of(&info, p),
                    abandoned_at: left_at(p),
                    mvp: p.mvp_rank() == 1,
                })
                .collect(),
        })
        .collect();
    Ok(Summary {
        match_id,
        date: date(i64::from(info.start_time())),
        mode: mode(&info),
        duration: mmss(u64::from(info.duration_s())),
        winner: team_name(winner),
        teams,
        sources: [Some(source), names_source]
            .into_iter()
            .flatten()
            .chain(cat.stale_sources())
            .collect(),
    })
}

// ---- match_player ----

#[derive(Serialize, JsonSchema)]
pub struct SoulSource {
    pub source: &'static str,
    /// Souls from kills and hits.
    pub souls: u32,
    /// Souls from orbs the player secured (lane troopers and neutrals drop them).
    pub orbs: u32,
}

#[derive(Serialize, JsonSchema)]
pub struct Unspent {
    /// Average souls held unspent over the match.
    pub avg_souls: u64,
    /// 1 = held the most unspent souls in the match.
    pub rank_in_match: usize,
    pub players: usize,
    /// Leavers top this list: their souls pile up while they are away.
    pub leavers_above: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct Death {
    pub at: String,
    pub killer: Option<String>,
    pub time_to_kill_s: Option<f64>,
    /// Map position (x, y) of the death.
    pub position: Option<[i64; 2]>,
    pub killer_position: Option<[i64; 2]>,
}

#[derive(Serialize, JsonSchema)]
pub struct Purchase {
    pub item: String,
    pub slot: Option<String>,
    pub tier: Option<i32>,
    pub bought_at: String,
    pub sold_at: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct PlayerReport {
    pub match_id: u64,
    pub account_id: u32,
    pub hero: String,
    pub team: &'static str,
    pub lane: Lane,
    pub result: PlayerResult,
    pub result_note: Option<&'static str>,
    pub abandoned_at: Option<String>,
    pub teammates_who_left: Vec<String>,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub net_worth: u32,
    pub last_hits: u32,
    pub denies: u32,
    pub hero_damage: u32,
    pub damage_taken: u32,
    pub healing: u32,
    pub souls_by_source: Vec<SoulSource>,
    pub souls_lost_on_death: u32,
    pub unspent: Option<Unspent>,
    /// Permanent stat buffs picked up (golden statues and the urn), by type.
    pub permanent_buffs: HashMap<String, u32>,
    pub deaths_detail: Vec<Death>,
    pub purchases: Vec<Purchase>,
    pub sources: Vec<Source>,
}

/// `custom_user_stats` values come as `{id, value}`; the names are listed once in `match_info`.
fn custom_stat(info: &MatchInfo, stat: &PlayerStats, name: &str) -> Option<u64> {
    let id = info
        .custom_user_stats
        .iter()
        .find(|s| s.name() == name)?
        .id();
    stat.custom_user_stats
        .iter()
        .find(|s| s.id() == id)
        .map(|s| u64::from(s.value()))
}

fn unspent_per_minute(info: &MatchInfo, p: &Players) -> Option<u64> {
    // The stat sums unspent souls over time in soul-minutes; per minute of the stat's own span.
    let stat = last_stat(p)?;
    let seconds = u64::from(stat.time_stamp_s());
    (seconds > 0)
        .then(|| custom_stat(info, stat, "Unspent Gold Minutes").map(|v| v * 60 / seconds))
        .flatten()
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "map coordinates fit in i64 and need no fraction"
)]
fn position(pos: Option<&Position>) -> Option<[i64; 2]> {
    let pos = pos?;
    Some([
        f64::from(pos.x?).round() as i64,
        f64::from(pos.y?).round() as i64,
    ])
}

/// Each death with the killer's hero and both positions.
fn deaths(cat: &Catalog, info: &MatchInfo, p: &Players) -> Vec<Death> {
    let slot_hero: HashMap<u32, String> = info
        .players
        .iter()
        .map(|q| (q.player_slot(), cat.hero(q.hero_id())))
        .collect();
    p.death_details
        .iter()
        .map(|d| Death {
            at: mmss(u64::from(d.game_time_s())),
            killer: d
                .killer_player_slot
                .and_then(|s| slot_hero.get(&s).cloned()),
            time_to_kill_s: d
                .time_to_kill_s
                .map(|t| (f64::from(t) * 10.0).round() / 10.0),
            position: position(d.death_pos.as_ref()),
            killer_position: position(d.killer_pos.as_ref()),
        })
        .collect()
}

/// Shop items bought (abilities and weapons are left out), with buy and sell times.
fn purchases(cat: &Catalog, p: &Players) -> Vec<Purchase> {
    p.items
        .iter()
        .filter_map(|i| {
            let item = cat
                .item(i.item_id())
                .filter(|item| item.kind == ItemKind::Upgrade)?;
            Some(Purchase {
                item: item.name.clone(),
                slot: item.slot.clone(),
                tier: item.tier,
                bought_at: mmss(u64::from(i.game_time_s())),
                sold_at: Some(i.sold_time_s())
                    .filter(|&s| s > 0)
                    .map(|s| mmss(u64::from(s))),
            })
        })
        .collect()
}

pub fn player(match_id: u64, account_id: u32) -> Result<PlayerReport> {
    let cat = catalog()?;
    let (info, source) = load(match_id)?;
    let p = find_player(&info, match_id, account_id)?;
    let no_stats = PlayerStats::default();
    let stat = last_stat(p).unwrap_or(&no_stats);

    let mut souls_by_source: Vec<SoulSource> = stat
        .gold_sources
        .iter()
        .map(|g| SoulSource {
            source: gold_source_name(g.source()),
            souls: g.gold(),
            orbs: g.gold_orbs(),
        })
        .filter(|s| s.souls + s.orbs > 0)
        .collect();
    souls_by_source.sort_by_key(|s| std::cmp::Reverse(s.souls + s.orbs));

    let unspent = unspent_per_minute(&info, p).map(|mine| {
        let all: Vec<(u64, bool)> = info
            .players
            .iter()
            .filter_map(|q| Some((unspent_per_minute(&info, q)?, left_at(q).is_some())))
            .collect();
        Unspent {
            avg_souls: mine,
            rank_in_match: all.iter().filter(|(v, _)| *v > mine).count() + 1,
            players: all.len(),
            leavers_above: all.iter().filter(|(v, left)| *left && *v > mine).count(),
        }
    });

    let deaths_detail = deaths(&cat, &info, p);

    let mut permanent_buffs: HashMap<String, u32> = HashMap::new();
    for b in p.power_up_buffs.iter().filter(|b| b.is_permanent()) {
        *permanent_buffs.entry(b.r#type().to_owned()).or_default() += b.value().max(1);
    }

    let purchases = purchases(&cat, p);

    Ok(PlayerReport {
        match_id,
        account_id,
        hero: cat.hero(p.hero_id()),
        team: team_name(p.team()),
        lane: cat.lane(p.assigned_lane()),
        result: result_of(&info, p),
        result_note: note(p.player_match_outcome.unwrap_or_default()),
        abandoned_at: left_at(p),
        teammates_who_left: info
            .players
            .iter()
            .filter(|q| q.team() == p.team() && q.account_id() != p.account_id())
            .filter_map(|q| left_at(q).map(|t| format!("{} at {t}", cat.hero(q.hero_id()))))
            .collect(),
        kills: p.kills(),
        deaths: p.deaths(),
        assists: p.assists(),
        net_worth: p.net_worth(),
        last_hits: p.last_hits(),
        denies: p.denies(),
        hero_damage: stat.player_damage(),
        damage_taken: stat.player_damage_taken(),
        healing: stat.player_healing(),
        souls_by_source,
        souls_lost_on_death: stat.gold_death_loss(),
        unspent,
        permanent_buffs,
        deaths_detail,
        purchases,
        sources: std::iter::once(source).chain(cat.stale_sources()).collect(),
    })
}

// ---- match_lanes ----

#[derive(Serialize, JsonSchema)]
pub struct LaneSample {
    pub at: String,
    /// Souls of the first team's duo minus the second's; positive = the first team is ahead.
    pub soul_lead: i64,
}

#[derive(Serialize, JsonSchema)]
pub struct LaneReport {
    pub lane: Lane,
    /// Heroes per team, in the order of `teams` in the answer.
    pub heroes: [Vec<String>; 2],
    pub soul_lead: Vec<LaneSample>,
}

#[derive(Serialize, JsonSchema)]
pub struct MatchLanes {
    pub match_id: u64,
    pub teams: [&'static str; 2],
    pub lanes: Vec<LaneReport>,
    pub sources: Vec<Source>,
}

fn net_worth_at(p: &Players, t: u32) -> u32 {
    p.stats
        .iter()
        .rev()
        .find(|s| s.time_stamp_s() <= t)
        .map_or(0, PlayerStats::net_worth)
}

pub fn lanes(match_id: u64) -> Result<MatchLanes> {
    let cat: std::sync::Arc<Catalog> = catalog()?;
    let (info, source) = load(match_id)?;
    let duration = info.duration_s();
    let lanes = cat
        .lanes()
        .iter()
        .map(|(lane_id, lane)| {
            let in_lane = |team: ECitadelLobbyTeam| {
                info.players
                    .iter()
                    .filter(move |p| p.assigned_lane() == *lane_id && p.team() == team)
            };
            let heroes = TEAMS.map(|team| in_lane(team).map(|p| cat.hero(p.hero_id())).collect());
            let total = |team: ECitadelLobbyTeam, t: u32| -> i64 {
                in_lane(team).map(|p| i64::from(net_worth_at(p, t))).sum()
            };
            let soul_lead = LANE_SAMPLES_S
                .iter()
                .filter(|&&t| t <= duration)
                .map(|&t| LaneSample {
                    at: mmss(u64::from(t)),
                    soul_lead: total(TEAMS[0], t) - total(TEAMS[1], t),
                })
                .collect();
            LaneReport {
                lane: lane.clone(),
                heroes,
                soul_lead,
            }
        })
        .collect();
    Ok(MatchLanes {
        match_id,
        teams: TEAMS.map(team_name),
        lanes,
        sources: std::iter::once(source).chain(cat.stale_sources()).collect(),
    })
}
