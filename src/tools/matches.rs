//! `match_summary`, `match_player` and `match_lanes`, from a match's metadata.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::valve::ECitadelLobbyTeam;
use crate::contracts::valve::c_msg_match_meta_data_contents::{
    MatchInfo, PlayerStats, Players, Position,
};
use crate::domain::assets::{Catalog, Lane, catalog};
use crate::domain::format::{date, mmss};
use crate::domain::items::ItemKind;
use crate::domain::outcome::{PlayerResult, note, player_result};
use crate::domain::terms::{game_mode_name, gold_source_name, match_mode_name, team_name};
use crate::http::Source;
use crate::problem::{Kind, Problem, Result};
use crate::sources::api;
use crate::tools::answer_sources;
use crate::tools::player::steam_names;

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
    let ids: Vec<u32> = info.players.iter().map(Players::account_id).collect();
    let (names, names_source) = steam_names(&ids);
    let sources = [Some(source), names_source]
        .into_iter()
        .flatten()
        .chain(cat.degraded_sources())
        .collect();
    Ok(summary_from(&cat, match_id, &info, &names, sources))
}

fn summary_from(
    cat: &Catalog,
    match_id: u64,
    info: &MatchInfo,
    names: &HashMap<u32, String>,
    sources: Vec<Source>,
) -> Summary {
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
                    hero_damage: last_stat(p).map_or_default(PlayerStats::player_damage),
                    result: result_of(info, p),
                    abandoned_at: left_at(p),
                    mvp: p.mvp_rank() == 1,
                })
                .collect(),
        })
        .collect();
    Summary {
        match_id,
        date: date(i64::from(info.start_time())),
        mode: mode(info),
        duration: mmss(u64::from(info.duration_s())),
        winner: team_name(winner),
        teams,
        sources,
    }
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
    /// The objective within about 50 m of `position`, if any.
    pub near: Option<String>,
    pub killer_position: Option<[i64; 2]>,
}

#[derive(Serialize, JsonSchema)]
pub struct NetWorthAt {
    pub at: String,
    pub net_worth: u32,
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
    /// At each stats snapshot, the times `performance_curve` averages; the last is the match end.
    pub net_worth_over_time: Vec<NetWorthAt>,
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
    /// Stats behind the end-of-match awards the game picked for this player, the earned ones first.
    /// Some appear nowhere else: crates broken (`breakables_destroyed`), damage by bullets, abilities
    /// or melee, headshots. Players get different subsets, so a missing stat means "not picked", not
    /// zero.
    pub awards: Vec<Award>,
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
            near: d
                .death_pos
                .as_ref()
                .and_then(|p| cat.near(f64::from(p.x?), f64::from(p.y?))),
            killer_position: position(d.killer_pos.as_ref()),
        })
        .collect()
}

#[derive(Serialize, JsonSchema)]
pub struct Award {
    /// The award's name in the game, e.g. "Championship Box-er".
    pub name: String,
    /// The stat it tracks, e.g. `breakables_destroyed`, `headshots`, `ability_damage`.
    pub stat: String,
    pub value: i32,
    /// The award's threshold the player reached, from 0; `None` when they reached none, so the award
    /// was not earned and `value` is only a stat. The game sets the thresholds, so a higher value
    /// does not always mean a higher level than another player's.
    pub level: Option<u8>,
}

/// The player's award stats, named from the game's award list, the earned ones first.
fn awards(cat: &Catalog, p: &Players) -> Vec<Award> {
    let mut awards: Vec<Award> = p
        .accolades
        .iter()
        .filter_map(|a| {
            let info = cat.accolade(a.accolade_id())?;
            Some(Award {
                name: info.flavor_name.clone(),
                stat: info.tracked_stat_name.clone(),
                value: a.accolade_stat_value(),
                // The game reports -1 for no threshold reached.
                level: u8::try_from(a.accolade_threshold_achieved()).ok(),
            })
        })
        .collect();
    awards.sort_by_key(|a| core::cmp::Reverse(a.level));
    awards
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
    let sources = answer_sources(source, &cat);
    player_from(&cat, match_id, account_id, &info, sources)
}

fn player_from(
    cat: &Catalog,
    match_id: u64,
    account_id: u32,
    info: &MatchInfo,
    sources: Vec<Source>,
) -> Result<PlayerReport> {
    let p = find_player(info, match_id, account_id)?;
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

    let unspent = unspent_per_minute(info, p).map(|mine| {
        let all: Vec<(u64, bool)> = info
            .players
            .iter()
            .filter_map(|q| Some((unspent_per_minute(info, q)?, left_at(q).is_some())))
            .collect();
        Unspent {
            avg_souls: mine,
            rank_in_match: all.iter().filter(|(v, _)| *v > mine).count() + 1,
            players: all.len(),
            leavers_above: all.iter().filter(|(v, left)| *left && *v > mine).count(),
        }
    });

    let deaths_detail = deaths(cat, info, p);

    let mut permanent_buffs: HashMap<String, u32> = HashMap::new();
    for b in p.power_up_buffs.iter().filter(|b| b.is_permanent()) {
        *permanent_buffs.entry(b.r#type().to_owned()).or_default() += b.value().max(1);
    }

    let purchases = purchases(cat, p);

    Ok(PlayerReport {
        match_id,
        account_id,
        hero: cat.hero(p.hero_id()),
        team: team_name(p.team()),
        lane: cat.lane(p.assigned_lane()),
        result: result_of(info, p),
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
        net_worth_over_time: p
            .stats
            .iter()
            .map(|s| NetWorthAt {
                at: mmss(u64::from(s.time_stamp_s())),
                net_worth: s.net_worth(),
            })
            .collect(),
        last_hits: p.last_hits(),
        denies: p.denies(),
        hero_damage: stat.player_damage(),
        damage_taken: stat.player_damage_taken(),
        healing: stat.player_healing(),
        souls_by_source,
        souls_lost_on_death: stat.gold_death_loss(),
        unspent,
        permanent_buffs,
        awards: awards(cat, p),
        deaths_detail,
        purchases,
        sources,
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
        .map_or_default(PlayerStats::net_worth)
}

pub fn lanes(match_id: u64) -> Result<MatchLanes> {
    let cat: std::sync::Arc<Catalog> = catalog()?;
    let (info, source) = load(match_id)?;
    let sources = answer_sources(source, &cat);
    Ok(lanes_from(&cat, match_id, &info, sources))
}

fn lanes_from(cat: &Catalog, match_id: u64, info: &MatchInfo, sources: Vec<Source>) -> MatchLanes {
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
    MatchLanes {
        match_id,
        teams: TEAMS.map(team_name),
        lanes,
        sources,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::contracts::api;
    use crate::contracts::valve::CMsgMatchMetaDataContents;
    use crate::domain::items::Item;

    /// Match 143768397 from `contracts/fixtures/match_metadata.json`.
    fn fixture() -> MatchInfo {
        let meta: CMsgMatchMetaDataContents =
            serde_json::from_str(include_str!("../../contracts/fixtures/match_metadata.json"))
                .unwrap();
        meta.match_info.unwrap()
    }

    /// The match's heroes as `H<id>`, its three lanes, the map, two items and three awards.
    fn cat() -> Catalog {
        let map: api::MapData =
            serde_json::from_str(include_str!("../../contracts/fixtures/map.json")).unwrap();
        let lane = |name: &str, color: &str| Lane {
            name: name.to_owned(),
            color: Some(color.to_owned()),
        };
        let lanes = vec![
            (1, lane("York", "#F1CC30")),
            (4, lane("Broadway", "#29B1CC")),
            (6, lane("Greenwich", "#59B247")),
        ];
        let award = |id, name: &str, stat: &str| api::Accolade {
            class_name: stat.to_owned(),
            description: String::new(),
            enabled_game_modes: None,
            flavor_name: name.to_owned(),
            id,
            threshold_type: "automatic".to_owned(),
            tracked_stat_name: stat.to_owned(),
        };
        let mut cat = Catalog::test(&[
            (3, "H3"),
            (13, "H13"),
            (20, "H20"),
            (27, "H27"),
            (63, "H63"),
            (64, "H64"),
            (66, "H66"),
            (72, "H72"),
            (76, "H76"),
            (78, "H78"),
            (79, "H79"),
            (84, "H84"),
        ])
        .with_map(lanes, &map)
        .with_accolades(vec![
            award(11, "Award 11", "stat_11"),
            award(21, "Award 21", "stat_21"),
            award(24, "Award 24", "stat_24"),
        ]);
        cat.items.insert(
            1_841_661_392,
            Item::test(1_841_661_392, "An ability", ItemKind::Ability),
        );
        let mut upgrade = Item::test(1_792_773_251, "An upgrade", ItemKind::Upgrade);
        upgrade.slot = Some("weapon".to_owned());
        upgrade.tier = Some(1);
        cat.items.insert(upgrade.id, upgrade);
        cat
    }

    const MATCH: u64 = 143_768_397;
    /// A player of the fixture: H72 in York for the winning team.
    const ME: u32 = 1_229_987_479;

    fn player_mut(info: &mut MatchInfo, account_id: u32) -> &mut Players {
        info.players
            .iter_mut()
            .find(|p| p.account_id() == account_id)
            .unwrap()
    }

    #[test]
    fn summary_splits_the_teams() {
        let names = HashMap::from([(ME, "a name".to_owned())]);
        let s = summary_from(&cat(), MATCH, &fixture(), &names, Vec::new());
        assert_eq!(
            (
                s.date.as_str(),
                s.mode.as_str(),
                s.duration.as_str(),
                s.winner
            ),
            ("2026-11-26", "Ranked (Normal)", "39:53", "The Hidden King")
        );
        // Account, Steam name, hero, lane, K/D/A, net worth, hero damage, result, MVP.
        let teams: Vec<_> = s
            .teams
            .iter()
            .map(|t| {
                let players: Vec<_> = t
                    .players
                    .iter()
                    .map(|p| {
                        format!(
                            "{} {:?} {} {} {}/{}/{} {} {} {:?}{}",
                            p.account_id,
                            p.steam_name,
                            p.hero,
                            p.lane.name,
                            p.kills,
                            p.deaths,
                            p.assists,
                            p.net_worth,
                            p.hero_damage,
                            p.result,
                            if p.mvp { " MVP" } else { "" }
                        )
                    })
                    .collect();
                players
            })
            .collect();
        let sides: Vec<_> = s.teams.iter().map(|t| (t.team, t.won)).collect();
        assert_eq!(
            sides,
            [("The Hidden King", true), ("The Archmother", false)]
        );
        assert_eq!(
            teams,
            [
                vec![
                    "1229987479 Some(\"a name\") H72 York 16/8/22 62191 65822 Win",
                    "1102668431 None H76 Broadway 5/12/22 46055 38148 Win",
                    "473049496 None H3 Greenwich 9/3/17 47920 30454 Win",
                    "1177670709 None H78 Broadway 10/10/15 41195 37948 Win",
                    "171043586 None H66 Greenwich 9/4/13 44991 23459 Win",
                    "1543524614 None H20 York 15/3/26 56166 49835 Win MVP",
                ],
                vec![
                    "879179282 None H13 Greenwich 9/13/4 41233 27860 Loss",
                    "85463520 None H84 Broadway 1/9/17 44267 17728 Loss",
                    "475303374 None H79 York 5/8/16 45237 27524 Loss",
                    "1221467105 None H63 York 8/9/10 45376 44946 Loss",
                    "200703287 None H64 Greenwich 8/11/16 45950 46753 Loss",
                    "119743951 None H27 Broadway 9/14/6 43364 36061 Loss",
                ],
            ]
        );
    }

    #[test]
    fn player_report_from_the_match() {
        let mut info = fixture();
        // A teammate who left, with the most unspent souls; an unearned award; a sold item.
        player_mut(&mut info, 1_543_524_614).abandon_match_time_s = Some(1500);
        let me = player_mut(&mut info, ME);
        me.accolades[0].accolade_threshold_achieved = Some(-1);
        me.items[2].sold_time_s = Some(600);
        let r = player_from(&cat(), MATCH, ME, &info, Vec::new()).unwrap();
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(
            (r.hero.as_str(), r.team, r.lane.name.as_str(), r.result),
            ("H72", "The Hidden King", "York", PlayerResult::Win)
        );
        assert_eq!(r.teammates_who_left, ["H20 at 25:00"]);
        assert_eq!(
            (
                r.last_hits,
                r.denies,
                r.hero_damage,
                r.damage_taken,
                r.healing
            ),
            (212, 1, 65822, 62862, 23139)
        );
        assert_eq!(r.souls_lost_on_death, 4913);
        assert_eq!(
            v["net_worth_over_time"],
            json!([
                {"at": "35:00", "net_worth": 45871},
                {"at": "39:53", "net_worth": 62191},
            ])
        );
        assert_eq!(
            v["souls_by_source"],
            json!([
                {"source": "lane troopers", "souls": 13572, "orbs": 6172},
                {"source": "hero kills", "souls": 16411, "orbs": 0},
                {"source": "jungle (neutrals, incl. Sinner's Sacrifice)", "souls": 8860, "orbs": 0},
                {"source": "breakables (crates)", "souls": 7837, "orbs": 0},
                {"source": "assists", "souls": 6430, "orbs": 0},
                {"source": "objectives and bosses", "souls": 4052, "orbs": 0},
                {"source": "urn and rift", "souls": 2357, "orbs": 0},
                {"source": "team bonus", "souls": 433, "orbs": 0},
                {"source": "denies", "souls": 153, "orbs": 0},
            ])
        );
        // 144113 soul-minutes over 39:53; only the leaver held more.
        assert_eq!(
            v["unspent"],
            json!({"avg_souls": 3613, "rank_in_match": 2, "players": 12, "leavers_above": 1})
        );
        assert_eq!(
            v["awards"],
            json!([
                {"name": "Award 24", "stat": "stat_24", "value": 26486, "level": 2},
                {"name": "Award 11", "stat": "stat_11", "value": 195, "level": 1},
                {"name": "Award 21", "stat": "stat_21", "value": 8, "level": null},
            ])
        );
        assert_eq!(
            v["purchases"],
            json!([{
                "item": "An upgrade",
                "slot": "weapon",
                "tier": 1,
                "bought_at": "1:43",
                "sold_at": "10:00",
            }])
        );
        assert_eq!(
            v["permanent_buffs"],
            json!({
                "ammo_permanent_pickup_lv2": 1,
                "bulletresist_permanent_pickup": 1,
                "bulletresist_permanent_pickup_lv2": 2,
                "cd_permanent_pickup_lv2": 2,
                "cd_permanent_pickup_lv3": 1,
                "firerate_permanent_pickup_lv2": 3,
                "firerate_permanent_pickup_lv3": 1,
                "hp_permanent_pickup_lv2": 2,
                "hp_permanent_pickup_lv3": 1,
                "movespeed_permanent_pickup_lv2": 2,
                "range_permanent_pickup_lv2": 1,
                "spirit_permanent_pickup": 1,
                "spirit_permanent_pickup_lv2": 3,
                "spiritresist_permanent_pickup_lv2": 1,
                "spiritresist_permanent_pickup_lv3": 1,
                "wp_permanent_pickup": 2,
            })
        );
    }

    #[test]
    fn deaths_name_killers_and_places() {
        let r = player_from(&cat(), MATCH, ME, &fixture(), Vec::new()).unwrap();
        let v = serde_json::to_value(&r.deaths_detail).unwrap();
        let york = "The Archmother's York Guardian";
        let death = |at, killer, ttk, pos: [i64; 2], near: Option<&str>, kpos: [i64; 2]| {
            json!({
                "at": at,
                "killer": killer,
                "time_to_kill_s": ttk,
                "position": pos,
                "near": near,
                "killer_position": kpos,
            })
        };
        assert_eq!(
            v,
            json!([
                death("1:58", "H79", 19.4, [-7061, 958], Some(york), [-6938, 978]),
                death(
                    "5:10",
                    "H63",
                    14.2,
                    [-6347, 2160],
                    Some(york),
                    [-7040, 1984]
                ),
                death("6:42", "H63", 18.3, [-4071, 2172], None, [-4172, 1331]),
                death("9:35", "H79", 18.6, [-6464, -523], None, [-6564, -409]),
                death(
                    "16:12",
                    "H79",
                    6.9,
                    [157, -1576],
                    Some("The Hidden King's Broadway Guardian"),
                    [116, -1519]
                ),
                death(
                    "27:41",
                    "H13",
                    10.4,
                    [6372, 2557],
                    Some("The Archmother's Greenwich Guardian"),
                    [6362, 2904]
                ),
                death(
                    "29:56",
                    "H13",
                    12.5,
                    [-5866, 3825],
                    Some("The Archmother's York Walker"),
                    [-5619, 4204]
                ),
                death(
                    "34:46",
                    "H13",
                    15.7,
                    [505, 6719],
                    Some("The Archmother's Patron"),
                    [794, 6812]
                ),
            ])
        );
    }

    #[test]
    fn player_must_be_in_the_match() {
        let err = player_from(&cat(), MATCH, 7, &fixture(), Vec::new())
            .err()
            .unwrap();
        assert_eq!(
            err.message(),
            "Invalid argument: account 7 didn't play match 143768397; match_summary lists its players"
        );
    }

    #[test]
    fn lanes_compare_the_duos() {
        let mut info = fixture();
        // A 10-minute match: samples at 3, 6 and 9 minutes. York's duos get early snapshots; the
        // fixture's own start at 35:00, so every other lane is even.
        info.duration_s = Some(600);
        let snapshot = |t, net_worth| PlayerStats {
            time_stamp_s: Some(t),
            net_worth: Some(net_worth),
            ..PlayerStats::default()
        };
        player_mut(&mut info, ME).stats = vec![snapshot(180, 1000), snapshot(360, 2500)];
        player_mut(&mut info, 475_303_374).stats = vec![snapshot(300, 1800)];
        let lanes = lanes_from(&cat(), MATCH, &info, Vec::new());
        assert_eq!(lanes.teams, ["The Hidden King", "The Archmother"]);
        let got: Vec<_> = lanes
            .lanes
            .iter()
            .map(|l| {
                let leads: Vec<_> = l
                    .soul_lead
                    .iter()
                    .map(|s| (s.at.as_str(), s.soul_lead))
                    .collect();
                (l.lane.name.as_str(), l.heroes.clone(), leads)
            })
            .collect();
        let even = vec![("3:00", 0), ("6:00", 0), ("9:00", 0)];
        let heroes = |a: [&str; 2], b: [&str; 2]| {
            [a.map(str::to_owned).to_vec(), b.map(str::to_owned).to_vec()]
        };
        assert_eq!(
            got,
            [
                (
                    "York",
                    heroes(["H72", "H20"], ["H79", "H63"]),
                    vec![("3:00", 1000), ("6:00", 700), ("9:00", 700)]
                ),
                (
                    "Broadway",
                    heroes(["H76", "H78"], ["H84", "H27"]),
                    even.clone()
                ),
                ("Greenwich", heroes(["H3", "H66"], ["H13", "H64"]), even),
            ]
        );
    }
}
