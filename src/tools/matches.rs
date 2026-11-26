//! `match_summary`, `match_player` and `match_lanes`, from a match's metadata.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::{date, mmss};
use crate::domain::outcome::{PlayerResult, note, player_result};
use crate::http::Source;
use crate::problem::{Kind, Problem, Result};
use crate::sources::api;

/// Lane ids as the game assigns them.
const LANES: [(u64, &str); 3] = [(1, "Yellow"), (4, "Blue"), (6, "Green")];
const TEAMS: [&str; 2] = ["The Hidden King", "The Archmother"];
/// Samples the per-player stats come in during the laning phase.
const LANE_SAMPLES_S: [u64; 5] = [180, 360, 540, 720, 900];

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn team_name(team: u64) -> String {
    usize::try_from(team)
        .ok()
        .and_then(|t| TEAMS.get(t))
        .map_or_else(|| format!("team {team}"), |t| (*t).to_owned())
}

fn lane_name(lane: u64) -> String {
    LANES
        .iter()
        .find(|(id, _)| *id == lane)
        .map_or_else(|| format!("lane {lane}"), |(_, n)| (*n).to_owned())
}

/// `match_mode` and `game_mode` as the game numbers them.
fn mode_name(match_mode: u64, game_mode: u64) -> String {
    let mode = match match_mode {
        1 => "Unranked",
        2 => "Private lobby",
        3 => "Bot match",
        4 => "Ranked",
        7 => "Hero Labs",
        8 => "Placement",
        _ => "Other",
    };
    if game_mode == 4 {
        format!("Street Brawl ({mode})")
    } else {
        mode.to_owned()
    }
}

struct Loaded {
    info: Value,
    source: Source,
}

fn load(match_id: u64) -> Result<Loaded> {
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
    let info = meta.get("match_info").cloned().ok_or_else(|| {
        Problem::new(
            Kind::InvalidResponse,
            format!("match {match_id} has no match_info"),
        )
        .with_instance(source.url.clone())
    })?;
    Ok(Loaded { info, source })
}

fn players(info: &Value) -> &[Value] {
    info["players"].as_array().map_or(&[], Vec::as_slice)
}

fn last_stat(p: &Value) -> &Value {
    p["stats"]
        .as_array()
        .and_then(|s| s.last())
        .unwrap_or(&Value::Null)
}

fn result_of(info: &Value, p: &Value) -> PlayerResult {
    player_result(
        u(&p["player_match_outcome"]),
        u(&p["team"]),
        u(&info["winning_team"]),
    )
}

fn left_at(p: &Value) -> Option<String> {
    Some(u(&p["abandon_match_time_s"]))
        .filter(|&s| s > 0)
        .map(mmss)
}

/// Steam names of the match's players; an empty map when the lookup fails.
fn steam_names(info: &Value) -> (HashMap<u64, String>, Option<Source>) {
    let ids: Vec<u32> = players(info)
        .iter()
        .filter_map(|p| u32::try_from(u(&p["account_id"])).ok())
        .collect();
    match api::steam_profiles(&ids) {
        Ok((profiles, source)) => {
            let names = profiles
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some((
                        p["account_id"].as_u64()?,
                        p["personaname"].as_str()?.to_owned(),
                    ))
                })
                .collect();
            (names, Some(source))
        }
        Err(_) => (HashMap::new(), None),
    }
}

fn find_player(info: &Value, match_id: u64, account_id: u32) -> Result<&Value> {
    players(info)
        .iter()
        .find(|p| u(&p["account_id"]) == u64::from(account_id))
        .ok_or_else(|| {
            Problem::invalid_argument(format!(
                "account {account_id} didn't play match {match_id}; match_summary lists its players"
            ))
        })
}

// ---- match_summary ----

#[derive(Serialize, JsonSchema)]
pub struct SummaryPlayer {
    pub account_id: u64,
    pub steam_name: Option<String>,
    pub hero: String,
    pub lane: String,
    pub kills: u64,
    pub deaths: u64,
    pub assists: u64,
    pub net_worth: u64,
    pub hero_damage: u64,
    /// This player's own result; differs from the team's when the game voided or penalized it.
    pub result: PlayerResult,
    /// Time the game flagged the player as having left (they may have reconnected).
    pub abandoned_at: Option<String>,
    pub mvp: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct SummaryTeam {
    pub team: String,
    pub won: bool,
    pub players: Vec<SummaryPlayer>,
}

#[derive(Serialize, JsonSchema)]
pub struct Summary {
    pub match_id: u64,
    pub date: String,
    pub mode: String,
    pub duration: String,
    pub winner: String,
    pub teams: Vec<SummaryTeam>,
    pub sources: Vec<Source>,
}

pub fn summary(match_id: u64) -> Result<Summary> {
    let cat = catalog()?;
    let Loaded { info, source } = load(match_id)?;
    let (names, names_source) = steam_names(&info);
    let winner = u(&info["winning_team"]);
    let teams = [0, 1]
        .into_iter()
        .map(|team| SummaryTeam {
            team: team_name(team),
            won: team == winner,
            players: players(&info)
                .iter()
                .filter(|p| u(&p["team"]) == team)
                .map(|p| SummaryPlayer {
                    account_id: u(&p["account_id"]),
                    steam_name: names.get(&u(&p["account_id"])).cloned(),
                    hero: cat.hero(u(&p["hero_id"])),
                    lane: lane_name(u(&p["assigned_lane"])),
                    kills: u(&p["kills"]),
                    deaths: u(&p["deaths"]),
                    assists: u(&p["assists"]),
                    net_worth: u(&p["net_worth"]),
                    hero_damage: u(&last_stat(p)["player_damage"]),
                    result: result_of(&info, p),
                    abandoned_at: left_at(p),
                    mvp: u(&p["mvp_rank"]) == 1,
                })
                .collect(),
        })
        .collect();
    Ok(Summary {
        match_id,
        date: date(info["start_time"].as_i64().unwrap_or(0)),
        mode: mode_name(u(&info["match_mode"]), u(&info["game_mode"])),
        duration: mmss(u(&info["duration_s"])),
        winner: team_name(winner),
        teams,
        sources: [Some(source), names_source].into_iter().flatten().collect(),
    })
}

// ---- match_player ----

/// `EGoldSource` in Valve's match metadata.
fn gold_source_name(source: u64) -> &'static str {
    match source {
        1 => "hero kills",
        2 => "lane troopers",
        3 => "jungle (neutrals)",
        4 => "objectives and bosses",
        5 => "urn and rift",
        6 => "assists",
        7 => "denies",
        8 => "team bonus",
        9 => "assassinate ability",
        10 => "Trophy Collector",
        11 => "Cultist Sacrifice",
        12 => "breakables (crates)",
        13 => "Golden Goose Egg",
        _ => "other",
    }
}

#[derive(Serialize, JsonSchema)]
pub struct SoulSource {
    pub source: &'static str,
    /// Souls from kills and hits.
    pub souls: u64,
    /// Souls from orbs the player secured (lane troopers and neutrals drop them).
    pub orbs: u64,
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
    pub tier: Option<u64>,
    pub bought_at: String,
    pub sold_at: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct PlayerReport {
    pub match_id: u64,
    pub account_id: u32,
    pub hero: String,
    pub team: String,
    pub lane: String,
    pub result: PlayerResult,
    pub result_note: Option<&'static str>,
    pub abandoned_at: Option<String>,
    pub teammates_who_left: Vec<String>,
    pub kills: u64,
    pub deaths: u64,
    pub assists: u64,
    pub net_worth: u64,
    pub last_hits: u64,
    pub denies: u64,
    pub hero_damage: u64,
    pub damage_taken: u64,
    pub healing: u64,
    pub souls_by_source: Vec<SoulSource>,
    pub souls_lost_on_death: u64,
    pub unspent: Option<Unspent>,
    /// Permanent stat buffs picked up (golden statues and the urn), by type.
    pub permanent_buffs: HashMap<String, u64>,
    pub deaths_detail: Vec<Death>,
    pub purchases: Vec<Purchase>,
    pub sources: Vec<Source>,
}

/// `custom_user_stats` values come as `{id, value}`; the names are listed once in `match_info`.
fn custom_stat(info: &Value, stat: &Value, name: &str) -> Option<u64> {
    let id = info["custom_user_stats"]
        .as_array()?
        .iter()
        .find(|s| s["name"] == name)?["id"]
        .as_u64()?;
    stat["custom_user_stats"]
        .as_array()?
        .iter()
        .find(|s| s["id"].as_u64() == Some(id))?["value"]
        .as_u64()
}

fn unspent_per_minute(info: &Value, p: &Value) -> Option<u64> {
    // The stat sums unspent souls over time in soul-minutes; per minute of the stat's own span.
    let stat = last_stat(p);
    let seconds = u(&stat["time_stamp_s"]);
    (seconds > 0)
        .then(|| custom_stat(info, stat, "Unspent Gold Minutes").map(|v| v * 60 / seconds))
        .flatten()
}

fn position(v: &Value) -> Option<[i64; 2]> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "map coordinates fit in i64 and need no fraction"
    )]
    Some([
        v["x"].as_f64()?.round() as i64,
        v["y"].as_f64()?.round() as i64,
    ])
}

pub fn player(match_id: u64, account_id: u32) -> Result<PlayerReport> {
    let cat = catalog()?;
    let Loaded { info, source } = load(match_id)?;
    let p = find_player(&info, match_id, account_id)?;
    let stat = last_stat(p);
    let team = u(&p["team"]);

    let mut souls_by_source: Vec<SoulSource> = stat["gold_sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| SoulSource {
            source: gold_source_name(u(&g["source"])),
            souls: u(&g["gold"]),
            orbs: u(&g["gold_orbs"]),
        })
        .filter(|s| s.souls + s.orbs > 0)
        .collect();
    souls_by_source.sort_by_key(|s| std::cmp::Reverse(s.souls + s.orbs));

    let unspent = unspent_per_minute(&info, p).map(|mine| {
        let all: Vec<(u64, bool)> = players(&info)
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

    let slot_hero: HashMap<u64, String> = players(&info)
        .iter()
        .map(|q| (u(&q["player_slot"]), cat.hero(u(&q["hero_id"]))))
        .collect();
    let deaths_detail = p["death_details"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| Death {
            at: mmss(u(&d["game_time_s"])),
            killer: d["killer_player_slot"]
                .as_u64()
                .and_then(|s| slot_hero.get(&s).cloned()),
            time_to_kill_s: d["time_to_kill_s"]
                .as_f64()
                .map(|t| (t * 10.0).round() / 10.0),
            position: position(&d["death_pos"]),
            killer_position: position(&d["killer_pos"]),
        })
        .collect();

    let mut permanent_buffs: HashMap<String, u64> = HashMap::new();
    for b in p["power_up_buffs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["is_permanent"] == true)
    {
        *permanent_buffs
            .entry(b["type"].as_str().unwrap_or("unknown").to_owned())
            .or_default() += u(&b["value"]).max(1);
    }

    let purchases = p["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let item = cat
                .item(u(&i["item_id"]))
                .filter(|item| item.kind == "upgrade")?;
            Some(Purchase {
                item: item.name.clone(),
                slot: item.slot.clone(),
                tier: item.tier,
                bought_at: mmss(u(&i["game_time_s"])),
                sold_at: Some(u(&i["sold_time_s"])).filter(|&s| s > 0).map(mmss),
            })
        })
        .collect();

    Ok(PlayerReport {
        match_id,
        account_id,
        hero: cat.hero(u(&p["hero_id"])),
        team: team_name(team),
        lane: lane_name(u(&p["assigned_lane"])),
        result: result_of(&info, p),
        result_note: note(u(&p["player_match_outcome"])),
        abandoned_at: left_at(p),
        teammates_who_left: players(&info)
            .iter()
            .filter(|q| u(&q["team"]) == team && q["account_id"] != p["account_id"])
            .filter_map(|q| left_at(q).map(|t| format!("{} at {t}", cat.hero(u(&q["hero_id"])))))
            .collect(),
        kills: u(&p["kills"]),
        deaths: u(&p["deaths"]),
        assists: u(&p["assists"]),
        net_worth: u(&p["net_worth"]),
        last_hits: u(&p["last_hits"]),
        denies: u(&p["denies"]),
        hero_damage: u(&stat["player_damage"]),
        damage_taken: u(&stat["player_damage_taken"]),
        healing: u(&stat["player_healing"]),
        souls_by_source,
        souls_lost_on_death: u(&stat["gold_death_loss"]),
        unspent,
        permanent_buffs,
        deaths_detail,
        purchases,
        sources: vec![source],
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
pub struct Lane {
    pub lane: String,
    /// Heroes per team, in the order of `teams` in the answer.
    pub heroes: [Vec<String>; 2],
    pub soul_lead: Vec<LaneSample>,
}

#[derive(Serialize, JsonSchema)]
pub struct Lanes {
    pub match_id: u64,
    pub teams: [String; 2],
    pub lanes: Vec<Lane>,
    pub sources: Vec<Source>,
}

fn net_worth_at(p: &Value, t: u64) -> u64 {
    p["stats"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|s| u(&s["time_stamp_s"]) <= t)
        .map_or(0, |s| u(&s["net_worth"]))
}

pub fn lanes(match_id: u64) -> Result<Lanes> {
    let cat: std::sync::Arc<Catalog> = catalog()?;
    let Loaded { info, source } = load(match_id)?;
    let duration = u(&info["duration_s"]);
    let lanes = LANES
        .iter()
        .map(|&(lane_id, name)| {
            let in_lane = |team: u64| {
                players(&info)
                    .iter()
                    .filter(move |p| u(&p["assigned_lane"]) == lane_id && u(&p["team"]) == team)
            };
            let heroes =
                [0, 1].map(|team| in_lane(team).map(|p| cat.hero(u(&p["hero_id"]))).collect());
            let total = |team: u64, t: u64| -> i64 {
                in_lane(team)
                    .map(|p| i64::try_from(net_worth_at(p, t)).unwrap_or(0))
                    .sum()
            };
            let soul_lead = LANE_SAMPLES_S
                .iter()
                .filter(|&&t| t <= duration)
                .map(|&t| LaneSample {
                    at: mmss(t),
                    soul_lead: total(0, t) - total(1, t),
                })
                .collect();
            Lane {
                lane: name.to_owned(),
                heroes,
                soul_lead,
            }
        })
        .collect();
    Ok(Lanes {
        match_id,
        teams: [team_name(0), team_name(1)],
        lanes,
        sources: vec![source],
    })
}
