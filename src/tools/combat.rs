//! `match_damage` and `match_distance`, from a match's damage matrix and player paths.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::valve::c_msg_match_meta_data_contents::{Deaths, MatchInfo, Players};
use crate::contracts::valve::c_msg_match_player_damage_matrix::EStatType;
use crate::contracts::valve::c_msg_match_player_paths_data::Path;
use crate::contracts::valve::{CMsgMatchPlayerDamageMatrix, CMsgMatchPlayerPathsData};
use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::mmss;
use crate::domain::map::{self, UNITS_PER_M};
use crate::http::Source;
use crate::problem::{Problem, Result};
use crate::tools::answer_sources;
use crate::tools::matches::{find_player, load};

/// The game records each hit twice: once under one of these groups and once under its specific
/// source. The groups are the game's (Scrap Grenade counts as `Bullet`), not damage types.
const GROUPS: [&str; 5] = ["Bullet", "Ability", "Melee", "Misc", "UnknownAbility"];
const CROWD_CONTROL: &str = "Citadel_Graph_CrowdControl_";
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
            let name =
                (stat == Some(EStatType::KETypeDamage) && !group).then(|| source_name(cat, raw));
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
                        *by.entry(name.clone().unwrap_or_default()).or_default() += value;
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
    if times.is_empty() {
        return Err(missing(match_id, "damage samples"));
    }
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
    /// Every other hero with a path, enemies first, then by seconds within 30 m. Positions come
    /// once a second; each path is lined up with game time on its player's deaths, so a few
    /// seconds of error remain around them.
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

/// How a player's path lines up with game time: sample = `rate` × second + `shift`. A path can
/// start late (a player who loaded late), stop early (one who disconnected) or hold more samples
/// than the match has seconds, so it is anchored on the player's deaths: the first sample with
/// health 0 after each death falls at that death's `game_time_s`.
struct Clock {
    rate: f64,
    shift: f64,
}

impl Clock {
    fn new(path: &Path, deaths: &[u32], duration: u32) -> Self {
        let zeros = path
            .health
            .windows(2)
            .zip(1u32..)
            .filter(|(w, _)| w[0] > 0 && w[1] == 0)
            .map(|(_, i)| f64::from(i));
        let mut deaths = deaths.to_vec();
        deaths.sort_unstable();
        let pairs: Vec<(f64, f64)> = deaths.into_iter().map(f64::from).zip(zeros).collect();
        let n = f64::from(u32::try_from(pairs.len()).unwrap_or(u32::MAX));
        let mean = |f: fn(&(f64, f64)) -> f64| pairs.iter().map(f).sum::<f64>() / n;
        match pairs.len() {
            // No deaths to anchor on: spread the path evenly over the match.
            0 => {
                let samples = u32::try_from(path.health.len().saturating_sub(1)).unwrap_or(0);
                Self {
                    rate: f64::from(samples) / f64::from(duration.max(1)),
                    shift: 0.0,
                }
            }
            1 => Self {
                rate: 1.0,
                shift: pairs[0].1 - pairs[0].0,
            },
            // Least squares over the deaths.
            _ => {
                let (t, s) = (mean(|p| p.0), mean(|p| p.1));
                let spread: f64 = pairs.iter().map(|p| (p.0 - t).powi(2)).sum();
                let rate = if spread > 0.0 {
                    pairs.iter().map(|p| (p.0 - t) * (p.1 - s)).sum::<f64>() / spread
                } else {
                    1.0
                };
                Self {
                    rate,
                    shift: s - rate * t,
                }
            }
        }
    }

    /// The sample at game second `t`, `None` outside the path.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a whole number checked to lie in 0..len"
    )]
    fn sample(&self, t: u32, len: usize) -> Option<usize> {
        let i = (self.rate * f64::from(t) + self.shift).round();
        (i >= 0.0 && i < f64::from(u32::try_from(len).ok()?)).then_some(i as usize)
    }
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
    let duration = info.duration_s();
    let clock = |q: &Players, path: &Path| {
        let deaths: Vec<u32> = q.death_details.iter().map(Deaths::game_time_s).collect();
        Clock::new(path, &deaths, duration)
    };
    let mine = path_of(p).ok_or_else(|| missing(match_id, "path of this player"))?;
    let my_clock = clock(p, mine);
    let from = from_minute.saturating_mul(60);
    let until = until_minute.map_or(duration, |m| m.saturating_mul(60).min(duration));
    if from >= duration {
        return Err(Problem::invalid_argument(format!(
            "match {match_id} lasted {}: from_minute must be before its end",
            mmss(u64::from(duration))
        )));
    }
    if from >= until {
        return Err(Problem::invalid_argument(
            "the window is empty: from_minute must be before until_minute",
        ));
    }
    let mut players: Vec<Proximity> = info
        .players
        .iter()
        .filter(|q| q.player_slot() != p.player_slot())
        .filter_map(|q| {
            let theirs = path_of(q)?;
            let their_clock = clock(q, theirs);
            let (mut both, mut near) = (0, [0; NEAR_M.len()]);
            for t in from..until {
                let at = |path: &Path, clock: &Clock| {
                    position(paths, path, clock.sample(t, path.health.len())?)
                };
                let (Some(a), Some(b)) = (at(mine, &my_clock), at(theirs, &their_clock)) else {
                    continue;
                };
                both += 1;
                let m = map::distance(a, b) / UNITS_PER_M;
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
        let m = map::distance(at, [f64::from(death.x()), f64::from(death.y())]) / UNITS_PER_M;
        assert!(m < 15.0, "{m} m");
    }

    /// A path whose health drops to 0 at these samples.
    fn path_dying_at(len: usize, zeros: &[usize]) -> Path {
        let mut health = vec![100; len];
        for &z in zeros {
            health[z] = 0;
        }
        Path {
            health,
            ..Path::default()
        }
    }

    #[test]
    fn clock_follows_the_deaths() {
        // Running slow (more samples than seconds), starting late and stopping early, as seen in
        // real matches: each death lands on its sample, and seconds off the path have none.
        let slow = path_dying_at(1431, &[163, 795, 1393]);
        let c = Clock::new(&slow, &[162, 782, 1366], 1403);
        // A straight line through three deaths: each within a second.
        for (t, at) in [(162, 163), (782, 795), (1366, 1393)] {
            assert!(c.sample(t, 1431).unwrap().abs_diff(at) <= 1, "{t}");
        }
        let late = path_dying_at(1746, &[256, 1415]);
        let c = Clock::new(&late, &[340, 1499], 2330);
        assert_eq!((c.sample(340, 1746), c.sample(10, 1746)), (Some(256), None));
        let early = path_dying_at(917, &[300]);
        let c = Clock::new(&early, &[299], 1635);
        assert_eq!((c.sample(299, 917), c.sample(1200, 917)), (Some(300), None));
        // Without deaths the path is spread over the match.
        let c = Clock::new(&path_dying_at(2531, &[]), &[], 2530);
        assert_eq!(c.sample(1500, 2531), Some(1500));
    }

    /// H79's path in the fixture's match runs ahead of the game clock (2440 samples over 39:53);
    /// the fixture keeps its first 15 minutes, with one death to anchor on.
    #[test]
    fn fixture_clock_lands_on_deaths() {
        let info = fixture();
        let paths = info.match_paths.as_ref().unwrap();
        let p = info.players.iter().find(|p| p.player_slot() == 12).unwrap();
        let path = paths.paths.iter().find(|x| x.player_slot() == 12).unwrap();
        let deaths: Vec<u32> = p.death_details.iter().map(Deaths::game_time_s).collect();
        let c = Clock::new(path, &deaths, info.duration_s());
        let zeros: Vec<usize> = (1..path.health.len())
            .filter(|&i| path.health[i - 1] > 0 && path.health[i] == 0)
            .collect();
        assert_eq!(zeros.len(), 1);
        for (&t, &z) in deaths.iter().zip(&zeros) {
            let at = c.sample(t, path.health.len()).unwrap();
            assert!(
                at.abs_diff(z) <= 2,
                "death at {t}: sample {at}, health 0 at {z}"
            );
        }
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
                ("H79".to_owned(), false, 806, [35, 103, 197, 330, 382]),
                ("H20".to_owned(), true, 853, [96, 235, 351, 420, 485]),
            ]
        );
        assert_eq!(
            got(&run(5, Some(10))),
            [
                ("H79".to_owned(), false, 262, [15, 30, 49, 92, 122]),
                ("H20".to_owned(), true, 262, [17, 46, 73, 106, 136]),
            ]
        );
        let err = |from, until| {
            distance_from(&cat(), MATCH, ME, &info, from, until, Vec::new())
                .err()
                .unwrap()
                .message()
        };
        assert!(err(5, Some(5)).contains("the window is empty"));
        assert!(err(15, None).contains("before its end"));
    }
}
