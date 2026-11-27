//! `match_damage` and `match_distance`, from a match's damage matrix and player paths.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::valve::c_msg_match_meta_data_contents::{MatchInfo, Players};
use crate::contracts::valve::c_msg_match_player_damage_matrix::EStatType;
use crate::contracts::valve::c_msg_match_player_paths_data::Path;
use crate::contracts::valve::{CMsgMatchPlayerDamageMatrix, CMsgMatchPlayerPathsData};
use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::mmss;
use crate::http::Source;
use crate::problem::{Problem, Result};
use crate::tools::answer_sources;
use crate::tools::matches::{find_player, load};

/// The game records each hit twice: once under one of these groups and once under its specific
/// source. The groups are the game's (Scrap Grenade counts as `Bullet`), not damage types.
const GROUPS: [&str; 5] = ["Bullet", "Ability", "Melee", "Misc", "UnknownAbility"];
const CROWD_CONTROL: &str = "Citadel_Graph_CrowdControl_";
/// World units per meter.
const UNITS_PER_M: f64 = 39.37;
const NEAR_M: [f64; 5] = [10.0, 20.0, 30.0, 45.0, 60.0];

fn missing(match_id: u64, what: &str) -> Problem {
    Problem::not_found(format!(
        "match {match_id} has no {what} in the Deadlock API's data"
    ))
}

// ---- match_damage ----

#[derive(Serialize, JsonSchema)]
pub struct SourceDamage {
    /// An ability, weapon or item; "(crits)" is the extra of critical hits, "(damage over time)"
    /// what an ability's effect dealt after the hit. A raw game name is one the catalog lacks.
    pub source: String,
    pub damage: u32,
}

#[derive(Serialize, JsonSchema)]
pub struct DamageBetween {
    pub hero: String,
    pub ally: bool,
    /// Damage this hero dealt to the player.
    pub received: u32,
    /// Damage the player dealt to this hero.
    pub dealt: u32,
    /// `received` by what dealt it, most first.
    pub received_by_source: Vec<SourceDamage>,
    /// `dealt` by what dealt it, most first.
    pub dealt_by_source: Vec<SourceDamage>,
    /// This hero's damage in the sequences that killed the player (the game's lethal damage): about
    /// the player's health per death they took part in, 0 when they took part in none.
    pub killing_damage: u32,
    /// Crowd control this hero put on the player, by type (slow, stun, root, silence, sleep,
    /// tether...). The game doesn't document the unit (it looks like milliseconds); compare it
    /// between heroes, not with seconds.
    pub crowd_control: BTreeMap<String, u32>,
}

#[derive(Serialize, JsonSchema)]
pub struct DamageReport {
    pub match_id: u64,
    pub account_id: u32,
    pub hero: String,
    /// The game time the numbers run up to: the game samples damage every 3 minutes and at the
    /// end, so this is the last sample at or before `until_minute`.
    pub until: String,
    /// Damage the player dealt to heroes; at the match end, `match_player`'s `hero_damage`.
    pub damage_dealt: u32,
    /// Damage heroes dealt to the player, not troopers, Guardians or neutrals; at the match end,
    /// `match_player`'s `damage_taken`.
    pub damage_received: u32,
    /// Heroes' damage in the sequences that killed the player.
    pub killing_damage_received: u32,
    /// Every enemy, and allies that damaged the player or were damaged by them, by `received`.
    pub players: Vec<DamageBetween>,
    pub sources: Vec<Source>,
}

pub fn damage(match_id: u64, account_id: u32, until_minute: Option<u32>) -> Result<DamageReport> {
    let cat = catalog()?;
    let (info, source) = load(match_id)?;
    let sources = answer_sources([source], &cat);
    damage_from(&cat, match_id, account_id, &info, until_minute, sources)
}

/// A source's name: the catalog's for a class name, also with the game's `_dot` and `_crit`
/// suffixes.
fn source_name(cat: &Catalog, raw: &str) -> String {
    let named = |class: &str| cat.item_by_class(class).map(|i| i.name.as_str());
    if let Some(name) = named(raw) {
        return name.to_owned();
    }
    let suffixed = [("_dot", "damage over time"), ("_crit", "crits")]
        .into_iter()
        .find_map(|(suffix, what)| {
            let name = named(raw.strip_suffix(suffix)?)?;
            Some(format!("{name} ({what})"))
        });
    suffixed.unwrap_or_else(|| raw.to_owned())
}

fn by_damage(sources: HashMap<String, u32>) -> Vec<SourceDamage> {
    let mut list: Vec<SourceDamage> = sources
        .into_iter()
        .filter(|(_, damage)| *damage > 0)
        .map(|(source, damage)| SourceDamage { source, damage })
        .collect();
    list.sort_by(|a, b| {
        b.damage
            .cmp(&a.damage)
            .then_with(|| a.source.cmp(&b.source))
    });
    list
}

#[derive(Default)]
struct Tally {
    received: u32,
    dealt: u32,
    received_by_source: HashMap<String, u32>,
    dealt_by_source: HashMap<String, u32>,
    killing_damage: u32,
    crowd_control: BTreeMap<String, u32>,
}

/// What the player and each other player (by slot) did to each other, at sample `k`.
fn tallies(
    cat: &Catalog,
    matrix: &CMsgMatchPlayerDamageMatrix,
    k: usize,
    players: &HashSet<u32>,
    me: u32,
) -> HashMap<u32, Tally> {
    let mut tallies: HashMap<u32, Tally> = HashMap::new();
    let Some(details) = &matrix.source_details else {
        return tallies;
    };
    for dealer in &matrix.damage_dealers {
        let from = dealer.dealer_player_slot();
        for source in &dealer.damage_sources {
            let i = source.source_details_index() as usize;
            let (Some(&stat), Some(raw)) = (details.stat_type.get(i), details.source_name.get(i))
            else {
                continue;
            };
            let stat = EStatType::try_from(stat).ok();
            let group = GROUPS.contains(&raw.as_str());
            for to in &source.damage_to_players {
                let target = to.target_player_slot();
                // Each series is cumulative and holds only its last samples: it starts at the first
                // sample with a value.
                let skipped = matrix.sample_time_s.len().saturating_sub(to.damage.len());
                let value = k
                    .checked_sub(skipped)
                    .and_then(|j| to.damage.get(j))
                    .copied()
                    .unwrap_or_default();
                // Other slots are troopers, Guardians and neutrals.
                if value == 0
                    || !players.contains(&from)
                    || !players.contains(&target)
                    || (from == me) == (target == me)
                {
                    continue;
                }
                let tally = tallies
                    .entry(if from == me { target } else { from })
                    .or_default();
                match stat {
                    Some(EStatType::KETypeDamage) if group && from == me => tally.dealt += value,
                    Some(EStatType::KETypeDamage) if group => tally.received += value,
                    Some(EStatType::KETypeDamage) => {
                        let by = if from == me {
                            &mut tally.dealt_by_source
                        } else {
                            &mut tally.received_by_source
                        };
                        *by.entry(source_name(cat, raw)).or_default() += value;
                    }
                    Some(EStatType::KETypeLethalDamage) if target == me => {
                        tally.killing_damage += value;
                    }
                    Some(EStatType::KETypeCrowdControl) if target == me => {
                        let kind = raw.strip_prefix(CROWD_CONTROL).unwrap_or(raw);
                        *tally.crowd_control.entry(kind.to_lowercase()).or_default() += value;
                    }
                    _ => {}
                }
            }
        }
    }
    tallies
}

fn damage_from(
    cat: &Catalog,
    match_id: u64,
    account_id: u32,
    info: &MatchInfo,
    until_minute: Option<u32>,
    sources: Vec<Source>,
) -> Result<DamageReport> {
    let p = find_player(info, match_id, account_id)?;
    let matrix = info
        .damage_matrix
        .as_ref()
        .ok_or_else(|| missing(match_id, "damage data"))?;
    let times = &matrix.sample_time_s;
    let limit = until_minute.map_or(u32::MAX, |m| m.saturating_mul(60));
    let k = times.iter().rposition(|&t| t <= limit).ok_or_else(|| {
        let first = times.first().copied().unwrap_or_default();
        Problem::invalid_argument(format!(
            "the first damage sample of match {match_id} is at {}; until_minute must be at least {}",
            mmss(u64::from(first)),
            first.div_ceil(60)
        ))
    })?;
    let me = p.player_slot();
    let slots: HashSet<u32> = info.players.iter().map(Players::player_slot).collect();
    let mut tallies = tallies(cat, matrix, k, &slots, me);

    let mut between: Vec<DamageBetween> = info
        .players
        .iter()
        .filter(|q| q.player_slot() != me)
        .filter_map(|q| {
            let ally = q.team() == p.team();
            let t = tallies.remove(&q.player_slot()).unwrap_or_default();
            if ally && t.received + t.dealt == 0 {
                return None;
            }
            Some(DamageBetween {
                hero: cat.hero(q.hero_id()),
                ally,
                received: t.received,
                dealt: t.dealt,
                received_by_source: by_damage(t.received_by_source),
                dealt_by_source: by_damage(t.dealt_by_source),
                killing_damage: t.killing_damage,
                crowd_control: t.crowd_control,
            })
        })
        .collect();
    between.sort_by_key(|b| Reverse(b.received));

    Ok(DamageReport {
        match_id,
        account_id,
        hero: cat.hero(p.hero_id()),
        until: mmss(u64::from(times[k])),
        damage_dealt: between.iter().map(|b| b.dealt).sum(),
        damage_received: between.iter().map(|b| b.received).sum(),
        killing_damage_received: between.iter().map(|b| b.killing_damage).sum(),
        players: between,
        sources,
    })
}

// ---- match_distance ----

#[derive(Serialize, JsonSchema)]
pub struct Proximity {
    pub hero: String,
    pub ally: bool,
    /// Seconds of the window in which both were alive; the other counts are out of these.
    pub both_alive_s: u32,
    /// Seconds within 10 m of each other. The counts are cumulative: `within_20m_s` includes these.
    pub within_10m_s: u32,
    pub within_20m_s: u32,
    pub within_30m_s: u32,
    pub within_45m_s: u32,
    pub within_60m_s: u32,
}

#[derive(Serialize, JsonSchema)]
pub struct DistanceReport {
    pub match_id: u64,
    pub account_id: u32,
    pub hero: String,
    /// The window, game time from and until.
    pub from: String,
    pub until: String,
    /// Every other hero, enemies first, then by seconds within 30 m. Positions come once a second;
    /// a player's path can hold more seconds than the match has, so each is stretched to the
    /// match length. A stretched path can still be up to about 20 s off the times in death lists,
    /// so read the seconds as rough shares of the window, not exact moments.
    pub players: Vec<Proximity>,
    pub sources: Vec<Source>,
}

pub fn distance(
    match_id: u64,
    account_id: u32,
    from_minute: u32,
    until_minute: Option<u32>,
) -> Result<DistanceReport> {
    let cat = catalog()?;
    let (info, source) = load(match_id)?;
    let sources = answer_sources([source], &cat);
    distance_from(
        &cat,
        match_id,
        account_id,
        &info,
        from_minute,
        until_minute,
        sources,
    )
}

/// The path sample at game second `t` of a match `duration` seconds long.
// shortcut: stretches each path evenly over the match, which leaves a path with extra samples up
// to ~20 s off mid-match (one with 137 extra over 40 min in match 143768397); anchor each path on
// its deaths (health reaching 0 at `death_details` times) if that ever matters.
fn sample_at(len: usize, duration: u32, t: u32) -> usize {
    let last = u64::try_from(len.saturating_sub(1)).unwrap_or_default();
    let duration = u64::from(duration.max(1));
    usize::try_from((u64::from(t) * last + duration / 2) / duration).unwrap_or(usize::MAX)
}

/// The world position (x, y) of sample `i`, or `None` while dead: the path then holds (0, 0).
fn position(paths: &CMsgMatchPlayerPathsData, path: &Path, i: usize) -> Option<[f64; 2]> {
    if *path.health.get(i)? == 0 {
        return None;
    }
    let axis = |pos: &[u32], resolution: u32, min: f32, max: f32| -> Option<f64> {
        let fraction = f64::from(*pos.get(i)?) / f64::from(resolution.max(1));
        Some(f64::from(min) + fraction * (f64::from(max) - f64::from(min)))
    };
    Some([
        axis(
            &path.x_pos,
            paths.x_resolution(),
            path.x_min(),
            path.x_max(),
        )?,
        axis(
            &path.y_pos,
            paths.y_resolution(),
            path.y_min(),
            path.y_max(),
        )?,
    ])
}

fn distance_from(
    cat: &Catalog,
    match_id: u64,
    account_id: u32,
    info: &MatchInfo,
    from_minute: u32,
    until_minute: Option<u32>,
    sources: Vec<Source>,
) -> Result<DistanceReport> {
    let p = find_player(info, match_id, account_id)?;
    let paths = info
        .match_paths
        .as_ref()
        .ok_or_else(|| missing(match_id, "player paths"))?;
    let path_of = |q: &Players| {
        paths
            .paths
            .iter()
            .find(|x| x.player_slot() == q.player_slot())
    };
    let mine = path_of(p).ok_or_else(|| missing(match_id, "path of this player"))?;
    let duration = info.duration_s();
    let from = from_minute.saturating_mul(60);
    let until = until_minute.map_or(duration, |m| m.saturating_mul(60).min(duration));
    if from >= until {
        return Err(Problem::invalid_argument(format!(
            "the window is empty: match {match_id} lasted {}, and from_minute must be before until_minute",
            mmss(u64::from(duration))
        )));
    }
    let mut players: Vec<Proximity> = info
        .players
        .iter()
        .filter(|q| q.player_slot() != p.player_slot())
        .filter_map(|q| {
            let theirs = path_of(q)?;
            let (mut both, mut near) = (0, [0; NEAR_M.len()]);
            for t in from..until {
                let at =
                    |path: &Path| position(paths, path, sample_at(path.x_pos.len(), duration, t));
                let (Some(a), Some(b)) = (at(mine), at(theirs)) else {
                    continue;
                };
                both += 1;
                let m = (a[0] - b[0]).hypot(a[1] - b[1]) / UNITS_PER_M;
                for (count, limit) in near.iter_mut().zip(NEAR_M) {
                    *count += u32::from(m <= limit);
                }
            }
            Some(Proximity {
                hero: cat.hero(q.hero_id()),
                ally: q.team() == p.team(),
                both_alive_s: both,
                within_10m_s: near[0],
                within_20m_s: near[1],
                within_30m_s: near[2],
                within_45m_s: near[3],
                within_60m_s: near[4],
            })
        })
        .collect();
    players.sort_by_key(|x| (x.ally, Reverse(x.within_30m_s)));
    Ok(DistanceReport {
        match_id,
        account_id,
        hero: cat.hero(p.hero_id()),
        from: mmss(u64::from(from)),
        until: mmss(u64::from(until)),
        players,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::valve::CMsgMatchMetaDataContents;
    use crate::domain::items::{Item, ItemKind};

    /// Match 143768397 from `contracts/fixtures/match_metadata.json`: damage dealt by H72 (`ME`)
    /// and H79, paths of H72, H20 and H79 for the first 900 seconds.
    fn fixture() -> MatchInfo {
        let meta: CMsgMatchMetaDataContents =
            serde_json::from_str(include_str!("../../contracts/fixtures/match_metadata.json"))
                .unwrap();
        meta.match_info.unwrap()
    }

    fn item(name: &str, class_name: &str, kind: ItemKind) -> Item {
        let mut item = Item::test(0, name, kind);
        item.class_name = class_name.to_owned();
        item
    }

    fn cat() -> Catalog {
        let heroes =
            [3, 13, 20, 27, 63, 64, 66, 72, 76, 78, 79, 84].map(|id| (id, format!("H{id}")));
        let heroes: Vec<(u32, &str)> = heroes.iter().map(|(id, n)| (*id, n.as_str())).collect();
        let mut cat = Catalog::test(&heroes);
        let items = [
            item("Biquette", "citadel_weapon_punkgoat_set", ItemKind::Weapon),
            item("Chain Gang", "ability_punkgoat_tether", ItemKind::Ability),
            item("Toxic Bullets", "upgrade_toxic_bullets", ItemKind::Upgrade),
        ];
        for (id, mut i) in (1..).zip(items) {
            i.id = id;
            cat.items.insert(id, i);
        }
        cat
    }

    const MATCH: u64 = 143_768_397;
    /// H72, player slot 5.
    const ME: u32 = 1_229_987_479;
    /// H79, slot 12, an enemy in ME's lane.
    const H79: u32 = 475_303_374;

    fn with<'a>(r: &'a DamageReport, hero: &str) -> &'a DamageBetween {
        r.players.iter().find(|b| b.hero == hero).unwrap()
    }

    #[test]
    fn damage_counts_each_hit_once() {
        let info = fixture();
        let matrix = info.damage_matrix.as_ref().unwrap();
        let details = matrix.source_details.as_ref().unwrap();
        for dealer in &matrix.damage_dealers {
            let slot = dealer.dealer_player_slot();
            let p = info
                .players
                .iter()
                .find(|p| p.player_slot() == slot)
                .unwrap();
            // The group rows to players (slots 1 to 12) add up to the game's own damage stat.
            let groups: u32 = dealer
                .damage_sources
                .iter()
                .filter(|s| {
                    let i = s.source_details_index() as usize;
                    details.stat_type[i] == 0 && GROUPS.contains(&details.source_name[i].as_str())
                })
                .flat_map(|s| &s.damage_to_players)
                .filter(|t| (1..=12).contains(&t.target_player_slot()))
                .filter_map(|t| t.damage.last())
                .sum();
            let stat = p.stats.last().unwrap().player_damage();
            assert_eq!(groups, stat);
            let r = damage_from(&cat(), MATCH, p.account_id(), &info, None, Vec::new()).unwrap();
            assert_eq!((r.until.as_str(), r.damage_dealt), ("39:53", stat));
            for b in &r.players {
                let by_source: u32 = b.dealt_by_source.iter().map(|s| s.damage).sum();
                assert_eq!(by_source, b.dealt, "{}", b.hero);
            }
        }
    }

    #[test]
    fn damage_between_two_players() {
        let info = fixture();
        let mine = |until| damage_from(&cat(), MATCH, ME, &info, until, Vec::new()).unwrap();
        // The series hold only their last samples; 4415 is H72's damage stat at 15:00.
        let lane = mine(Some(15));
        assert_eq!((lane.until.as_str(), lane.damage_dealt), ("15:00", 4415));
        assert_eq!(mine(Some(17)).until, "15:00");
        let h79 = with(&lane, "H79");
        assert_eq!((h79.received, h79.dealt, h79.ally), (2691, 1089, false));
        // H79 took part in two of H72's deaths by then (1:58 and 9:35).
        assert!(h79.killing_damage > 1000, "{}", h79.killing_damage);
        assert_eq!(h79.crowd_control.values().sum::<u32>(), 22374);
        // Only H79's own damage is in the fixture, so the other enemies dealt none; allies with
        // no damage either way are left out.
        assert_eq!(lane.damage_received, 2691);
        assert_eq!(lane.players[0].hero, "H79");
        assert_eq!(lane.players.len(), 6);

        // Both dealers' rows agree.
        let end = mine(None);
        let theirs = damage_from(&cat(), MATCH, H79, &info, None, Vec::new()).unwrap();
        assert_eq!(with(&end, "H79").received, with(&theirs, "H72").dealt);
        assert_eq!(with(&end, "H79").received, 8871);
        let sources: Vec<&str> = with(&end, "H79")
            .dealt_by_source
            .iter()
            .map(|s| s.source.as_str())
            .collect();
        assert!(sources.contains(&"Biquette"), "{sources:?}");
        assert!(sources.contains(&"Biquette (crits)"), "{sources:?}");

        let err = damage_from(&cat(), MATCH, ME, &info, Some(2), Vec::new())
            .err()
            .unwrap()
            .message();
        assert!(
            err.contains("first damage sample of match 143768397 is at 3:00"),
            "{err}"
        );
    }

    #[test]
    fn sources_are_named_from_the_catalog() {
        let cat = cat();
        assert_eq!(source_name(&cat, "ability_punkgoat_tether"), "Chain Gang");
        assert_eq!(
            source_name(&cat, "ability_punkgoat_tether_dot"),
            "Chain Gang (damage over time)"
        );
        assert_eq!(
            source_name(&cat, "citadel_weapon_punkgoat_set_crit"),
            "Biquette (crits)"
        );
        assert_eq!(source_name(&cat, "upgrade_toxic_bullets"), "Toxic Bullets");
        assert_eq!(source_name(&cat, "base_stat_regen"), "base_stat_regen");
    }

    #[test]
    fn path_positions_match_deaths() {
        let info = fixture();
        let paths = info.match_paths.as_ref().unwrap();
        let p = info.players.iter().find(|p| p.account_id() == ME).unwrap();
        let path = paths
            .paths
            .iter()
            .find(|x| x.player_slot() == p.player_slot())
            .unwrap();
        // H72 died at 1:58; the path's health is 0 from the next second, where the position is
        // not kept. The second before lies at the death.
        let dead = path.health.iter().position(|&h| h == 0).unwrap();
        assert_eq!(dead, 119);
        assert_eq!(position(paths, path, dead), None);
        let at = position(paths, path, dead - 1).unwrap();
        let death = p.death_details[0].death_pos.unwrap();
        let m = (at[0] - f64::from(death.x())).hypot(at[1] - f64::from(death.y())) / UNITS_PER_M;
        assert!(m < 15.0, "{m} m");
    }

    #[test]
    fn paths_are_stretched_to_the_match() {
        // Most paths hold one sample per second of the match.
        assert_eq!(sample_at(2394, 2393, 1500), 1500);
        // H63's path in this match holds 2530 samples; his death at 38:06 shows at sample 2417.
        assert!(sample_at(2530, 2393, 2286).abs_diff(2417) <= 2);
    }

    #[test]
    fn distances_to_other_players() {
        let mut info = fixture();
        // The fixture's paths stop at 15:00 (900 samples): a match of that length.
        info.duration_s = Some(899);
        let run =
            |from, until| distance_from(&cat(), MATCH, ME, &info, from, until, Vec::new()).unwrap();
        let got = |r: &DistanceReport| -> Vec<_> {
            r.players
                .iter()
                .map(|x| {
                    (
                        x.hero.clone(),
                        x.ally,
                        x.both_alive_s,
                        [
                            x.within_10m_s,
                            x.within_20m_s,
                            x.within_30m_s,
                            x.within_45m_s,
                            x.within_60m_s,
                        ],
                    )
                })
                .collect()
        };
        // Only H20 and H79 have paths in the fixture; enemies come first.
        let lane = run(0, Some(15));
        assert_eq!((lane.from.as_str(), lane.until.as_str()), ("0:00", "14:59"));
        assert_eq!(
            got(&lane),
            [
                ("H79".to_owned(), false, 820, [32, 116, 226, 313, 353]),
                ("H20".to_owned(), true, 853, [114, 267, 357, 421, 478]),
            ]
        );
        assert_eq!(
            got(&run(5, Some(10))),
            [
                ("H79".to_owned(), false, 262, [3, 18, 44, 63, 80]),
                ("H20".to_owned(), true, 262, [21, 55, 76, 102, 132]),
            ]
        );
        let err = distance_from(&cat(), MATCH, ME, &info, 15, Some(15), Vec::new())
            .err()
            .unwrap();
        assert!(err.message().contains("the window is empty"));
    }
}
