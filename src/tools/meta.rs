//! The meta from the Deadlock API's analytics over all players' matches: `hero_stats`,
//! `hero_items`, `hero_counters`, `hero_synergies`, `ability_order`; plus `builds` and
//! `leaderboard`.

use std::cmp::Reverse;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::contracts::api::{MetricValues, PlayerStatsMetricsKey as Metric};
use crate::contracts::valve::c_msg_match_meta_data_contents::EGoldSource as G;
use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::{bounded, date, mmss, plain};
use crate::domain::items::ItemKind;
use crate::domain::terms::gold_source_name;
use crate::http::Source;
use crate::problem::Result;
use crate::sources::api;
use crate::tools::player::steam_names;
use crate::tools::{Filters, round, take, win_rate};

fn id32(id: i64) -> u32 {
    u32::try_from(id).unwrap_or_default()
}

/// Win rate of a row for sorting; rows without matches sort last.
fn rate(wins: i64, matches: i64) -> f64 {
    win_rate(wins, matches).unwrap_or(0.0)
}

fn sources(source: Source, cat: &Catalog) -> Vec<Source> {
    std::iter::once(source).chain(cat.stale_sources()).collect()
}

#[derive(Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeroSort {
    /// Highest win rate first.
    #[default]
    WinRate,
    /// Most picked first.
    PickRate,
}

#[derive(Serialize, JsonSchema)]
pub struct HeroRow {
    pub hero: String,
    pub matches: i64,
    /// Wins over matches, 0..1.
    pub win_rate: Option<f64>,
    /// Share of all matches the hero was in, 0..1.
    pub pick_rate: f64,
    pub kills_per_match: f64,
    pub deaths_per_match: f64,
    pub assists_per_match: f64,
    pub souls_per_match: f64,
}

#[derive(Serialize, JsonSchema)]
pub struct HeroStats {
    /// Matches in the sample (each has 12 heroes).
    pub matches: i64,
    pub heroes: Vec<HeroRow>,
    pub sources: Vec<Source>,
}

#[expect(clippy::cast_precision_loss, reason = "counts are far below 2^52")]
fn per(total: i64, matches: i64) -> f64 {
    if matches == 0 {
        0.0
    } else {
        round(total as f64 / matches as f64, 2)
    }
}

pub fn hero_stats(
    filters: &Filters,
    min_matches: u32,
    sort: HeroSort,
    limit: u32,
) -> Result<HeroStats> {
    let cat = catalog()?;
    let (rows, source) = api::hero_stats(&filters.query())?;
    let matches = rows.iter().map(|h| h.matches).sum::<i64>() / 12;
    let mut heroes: Vec<_> = rows
        .iter()
        .filter(|h| h.matches >= i64::from(min_matches))
        .map(|h| HeroRow {
            hero: cat.hero(id32(h.hero_id.into())),
            matches: h.matches,
            win_rate: win_rate(h.wins, h.matches),
            pick_rate: win_rate(h.matches, matches).unwrap_or_default(),
            kills_per_match: per(h.total_kills, h.matches),
            deaths_per_match: per(h.total_deaths, h.matches),
            assists_per_match: per(h.total_assists, h.matches),
            souls_per_match: round(per(h.total_net_worth, h.matches), 0),
        })
        .collect();
    match sort {
        HeroSort::WinRate => heroes.sort_by(|a, b| {
            b.win_rate
                .unwrap_or_default()
                .total_cmp(&a.win_rate.unwrap_or_default())
        }),
        HeroSort::PickRate => heroes.sort_by_key(|h| Reverse(h.matches)),
    }
    heroes.truncate(take(limit));
    Ok(HeroStats {
        matches,
        heroes,
        sources: sources(source, &cat),
    })
}

#[derive(Serialize, JsonSchema)]
pub struct ItemRow {
    pub item: String,
    /// `weapon`, `spirit` or `vitality`.
    pub slot: Option<String>,
    pub tier: Option<i32>,
    pub cost: Option<i64>,
    /// Matches in which a player of the hero bought it.
    pub matches: i64,
    /// Wins over those matches, 0..1.
    pub win_rate: Option<f64>,
    /// Average game time of the purchase, `m:ss`.
    pub avg_buy_time: String,
}

#[derive(Serialize, JsonSchema)]
pub struct HeroItems {
    pub hero: String,
    /// Only matches against at least one of these heroes.
    pub enemy_heroes: Vec<String>,
    /// Highest win rate first. Items bought late win more often (games that go long were often
    /// already won), so compare items of one tier and look at the buy time.
    pub items: Vec<ItemRow>,
    pub sources: Vec<Source>,
}

pub fn hero_items(
    hero: &str,
    enemy_heroes: &[String],
    filters: &Filters,
    min_matches: u32,
    limit: u32,
) -> Result<HeroItems> {
    let cat = catalog()?;
    let hero_id = cat.find_hero(hero)?;
    let enemies = enemy_heroes
        .iter()
        .map(|h| cat.find_hero(h))
        .collect::<Result<Vec<u32>>>()?;
    let mut query = format!(
        "&hero_id={hero_id}&min_matches={min_matches}{}",
        filters.query()
    );
    if !enemies.is_empty() {
        let ids: Vec<String> = enemies.iter().map(u32::to_string).collect();
        let _ = write!(query, "&enemy_hero_ids={}", ids.join(","));
    }
    let (rows, source) = api::item_stats(&query)?;
    let mut items: Vec<_> = rows
        .iter()
        .filter_map(|r| {
            Some((
                r,
                cat.item(id32(r.item_id))
                    .filter(|i| i.kind == ItemKind::Upgrade)?,
            ))
        })
        .collect();
    items.sort_by(|a, b| rate(b.0.wins, b.0.matches).total_cmp(&rate(a.0.wins, a.0.matches)));
    Ok(HeroItems {
        hero: cat.hero(hero_id),
        enemy_heroes: enemies.iter().map(|&h| cat.hero(h)).collect(),
        items: items
            .into_iter()
            .take(take(limit))
            .map(|(r, i)| ItemRow {
                item: i.name.clone(),
                slot: i.slot.clone(),
                tier: i.tier,
                cost: i.cost,
                matches: r.matches,
                win_rate: win_rate(r.wins, r.matches),
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "game times are small and positive"
                )]
                avg_buy_time: mmss(r.avg_buy_time_s.max(0.0) as u64),
            })
            .collect(),
        sources: sources(source, &cat),
    })
}

/// Another hero and how matches with or against it went for the hero asked about.
#[derive(Serialize, JsonSchema)]
pub struct Pairing {
    pub hero: String,
    pub matches: i64,
    /// The asked-about hero's wins over those matches, 0..1.
    pub win_rate: Option<f64>,
}

/// `(other hero, wins, matches)` rows as the best and the worst `limit` pairings.
fn best_and_worst(
    cat: &Catalog,
    mut rows: Vec<(u32, i64, i64)>,
    limit: u32,
) -> (Vec<Pairing>, Vec<Pairing>) {
    rows.sort_by(|a, b| {
        rate(b.1, b.2)
            .total_cmp(&rate(a.1, a.2))
            .then(a.0.cmp(&b.0))
    });
    let pairing = |&(h, wins, matches): &(u32, i64, i64)| Pairing {
        hero: cat.hero(h),
        matches,
        win_rate: win_rate(wins, matches),
    };
    let best = rows.iter().take(take(limit)).map(pairing).collect();
    let worst = rows.iter().rev().take(take(limit)).map(pairing).collect();
    (best, worst)
}

fn pair_query(filters: &Filters, min_matches: u32, same_lane: bool) -> String {
    format!(
        "&min_matches={min_matches}&same_lane_filter={same_lane}{}",
        filters.query()
    )
}

#[derive(Serialize, JsonSchema)]
pub struct Counters {
    pub hero: String,
    /// Only enemies that laned against the hero.
    pub same_lane: bool,
    /// Enemies the hero wins most against, best first.
    pub best_against: Vec<Pairing>,
    /// Enemies the hero loses most against, worst first.
    pub worst_against: Vec<Pairing>,
    pub sources: Vec<Source>,
}

pub fn hero_counters(
    hero: &str,
    filters: &Filters,
    min_matches: u32,
    same_lane: bool,
    limit: u32,
) -> Result<Counters> {
    let cat = catalog()?;
    let hero_id = cat.find_hero(hero)?;
    let (rows, source) = api::hero_counter_stats(&pair_query(filters, min_matches, same_lane))?;
    let rows = rows
        .iter()
        .filter(|r| id32(r.hero_id.into()) == hero_id)
        .map(|r| (id32(r.enemy_hero_id.into()), r.wins, r.matches_played))
        .collect();
    let (best_against, worst_against) = best_and_worst(&cat, rows, limit);
    Ok(Counters {
        hero: cat.hero(hero_id),
        same_lane,
        best_against,
        worst_against,
        sources: sources(source, &cat),
    })
}

#[derive(Serialize, JsonSchema)]
pub struct Synergies {
    pub hero: String,
    /// Only teammates that shared the hero's lane.
    pub same_lane: bool,
    /// Teammates the hero wins most with, best first.
    pub best_with: Vec<Pairing>,
    /// Teammates the hero wins least with, worst first.
    pub worst_with: Vec<Pairing>,
    pub sources: Vec<Source>,
}

pub fn hero_synergies(
    hero: &str,
    filters: &Filters,
    min_matches: u32,
    same_lane: bool,
    limit: u32,
) -> Result<Synergies> {
    let cat = catalog()?;
    let hero_id = cat.find_hero(hero)?;
    let (rows, source) = api::hero_synergy_stats(&pair_query(filters, min_matches, same_lane))?;
    // Each pair comes once, in either order.
    let rows = rows
        .iter()
        .filter_map(|r| {
            let (a, b) = (id32(r.hero_id1.into()), id32(r.hero_id2.into()));
            let mate = if a == hero_id {
                b
            } else if b == hero_id {
                a
            } else {
                return None;
            };
            Some((mate, r.wins, r.matches_played))
        })
        .collect();
    let (best_with, worst_with) = best_and_worst(&cat, rows, limit);
    Ok(Synergies {
        hero: cat.hero(hero_id),
        same_lane,
        best_with,
        worst_with,
        sources: sources(source, &cat),
    })
}

#[derive(Serialize, JsonSchema)]
pub struct Order {
    /// Ability points in the order they were spent.
    pub abilities: Vec<String>,
    pub matches: i64,
    /// Wins over matches, 0..1.
    pub win_rate: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
pub struct AbilityOrders {
    pub hero: String,
    /// Most played orders first.
    pub most_common: Vec<Order>,
    /// Highest win rate first.
    pub best: Vec<Order>,
    pub sources: Vec<Source>,
}

pub fn ability_order(
    hero: &str,
    filters: &Filters,
    min_matches: u32,
    limit: u32,
) -> Result<AbilityOrders> {
    let cat = catalog()?;
    let hero_id = cat.find_hero(hero)?;
    let query = format!("&min_matches={min_matches}{}", filters.query());
    let (mut rows, source) = api::ability_order_stats(hero_id, &query)?;
    let order = |r: &crate::contracts::api::AnalyticsAbilityOrderStats| Order {
        abilities: r
            .abilities
            .iter()
            .map(|&a| {
                cat.item(id32(a))
                    .map_or_else(|| format!("ability {a}"), |i| i.name.clone())
            })
            .collect(),
        matches: r.matches,
        win_rate: win_rate(r.wins, r.matches),
    };
    rows.sort_by_key(|r| Reverse(r.matches));
    let most_common = rows.iter().take(take(limit)).map(order).collect();
    rows.sort_by(|a, b| rate(b.wins, b.matches).total_cmp(&rate(a.wins, a.matches)));
    let best = rows.iter().take(take(limit)).map(order).collect();
    Ok(AbilityOrders {
        hero: cat.hero(hero_id),
        most_common,
        best,
        sources: sources(source, &cat),
    })
}

#[derive(Serialize, JsonSchema)]
pub struct BuildRow {
    pub name: String,
    /// Search this id in the game's build browser to import the build.
    pub hero_build_id: i64,
    pub version: i64,
    pub author_account_id: i64,
    pub author: Option<String>,
    /// Favorites in the last week.
    pub weekly_favorites: Option<i64>,
    pub favorites: Option<i64>,
    pub last_updated: Option<String>,
    /// The author's description, cut at 300 characters.
    pub description: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct HeroBuilds {
    pub hero: String,
    /// Most favorited this week first.
    pub builds: Vec<BuildRow>,
    pub sources: Vec<Source>,
}

pub fn builds(
    hero: &str,
    search: Option<&str>,
    language: Option<crate::contracts::api::BuildLanguage>,
    limit: u32,
) -> Result<HeroBuilds> {
    let cat = catalog()?;
    let hero_id = cat.find_hero(hero)?;
    let mut query =
        format!("&hero_id={hero_id}&sort_by=weekly_favorites&sort_direction=desc&limit={limit}");
    if let Some(s) = search {
        let _ = write!(query, "&search_name={}", crate::http::encode(s));
    }
    if let Some(language) = language {
        let _ = write!(query, "&build_language={language}");
    }
    let (rows, source) = api::builds(&query)?;
    let ids: Vec<u32> = rows
        .iter()
        .map(|b| id32(b.hero_build.author_account_id))
        .collect();
    let (names, names_source) = steam_names(&ids);
    let builds = rows
        .iter()
        .take(take(limit))
        .map(|b| {
            let h = &b.hero_build;
            BuildRow {
                name: plain(&h.name),
                hero_build_id: h.hero_build_id,
                version: h.version,
                author_account_id: h.author_account_id,
                author: names.get(&id32(h.author_account_id)).cloned(),
                weekly_favorites: b.num_weekly_favorites,
                favorites: b.num_favorites,
                last_updated: h.last_updated_timestamp.map(date),
                description: h
                    .description
                    .as_deref()
                    .map(|d| bounded(plain(d).trim(), 300).0)
                    .filter(|d| !d.is_empty()),
            }
        })
        .collect();
    Ok(HeroBuilds {
        hero: cat.hero(hero_id),
        builds,
        sources: std::iter::once(source)
            .chain(names_source)
            .chain(cat.stale_sources())
            .collect(),
    })
}

/// Leaderboard regions, as the API names them.
#[derive(Clone, Copy, Deserialize, JsonSchema)]
pub enum Region {
    Europe,
    Asia,
    NAmerica,
    SAmerica,
    Oceania,
}

impl Region {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Europe => "Europe",
            Self::Asia => "Asia",
            Self::NAmerica => "NAmerica",
            Self::SAmerica => "SAmerica",
            Self::Oceania => "Oceania",
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub struct LeaderboardRow {
    /// Place on the leaderboard from 1; tied players share one.
    pub place: Option<i64>,
    pub name: Option<String>,
    /// Accounts that may be this player (matched by name, so not always right), at most 3.
    pub possible_account_ids: Vec<i64>,
    /// Their most played heroes, as the game reports them (often empty).
    pub top_heroes: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Leaderboard {
    pub region: &'static str,
    pub hero: Option<String>,
    pub players: Vec<LeaderboardRow>,
    pub sources: Vec<Source>,
}

pub fn leaderboard(region: Region, hero: Option<&str>, limit: u32) -> Result<Leaderboard> {
    let cat = catalog()?;
    let hero_id = hero.map(|h| cat.find_hero(h)).transpose()?;
    let (board, source) = api::leaderboard(region.as_str(), hero_id)?;
    Ok(Leaderboard {
        region: region.as_str(),
        hero: hero_id.map(|h| cat.hero(h)),
        players: board
            .entries
            .iter()
            .take(take(limit))
            .map(|e| LeaderboardRow {
                place: e.rank,
                name: e.account_name.as_deref().map(plain),
                possible_account_ids: e.possible_account_ids.iter().take(3).copied().collect(),
                top_heroes: e.top_hero_ids.iter().map(|&h| cat.hero(id32(h))).collect(),
            })
            .collect(),
        sources: sources(source, &cat),
    })
}

/// `&hero_ids=<id>` for an optional hero, and the hero's name for the answer.
fn hero_filter(cat: &Catalog, hero: Option<&str>) -> Result<(String, Option<String>)> {
    let id = hero.map(|h| cat.find_hero(h)).transpose()?;
    Ok((
        id.map_or_default(|h| format!("&hero_ids={h}")),
        id.map(|h| cat.hero(h)),
    ))
}

#[derive(Serialize, JsonSchema)]
pub struct CurveSouls {
    pub source: &'static str,
    /// Average souls from this source by then; orbs included (`match_player` lists them apart).
    pub souls: f64,
}

#[derive(Serialize, JsonSchema)]
pub struct CurvePoint {
    /// Game time of the stats snapshot, `m:ss`; `match_lanes` and `match_player` use the same ones.
    pub at: String,
    /// Player snapshots averaged here (one per player per match that lasted this long).
    pub samples: i64,
    /// Average net worth (all souls collected) by then.
    pub net_worth: f64,
    /// Standard deviation of net worth; about two thirds of players are within it of the average.
    pub net_worth_std: f64,
    pub kills: f64,
    pub deaths: f64,
    pub assists: f64,
    /// Lane troopers last-hit.
    pub last_hits: f64,
    pub denies: f64,
    /// Damage dealt to enemy heroes.
    pub hero_damage: f64,
    pub healing: f64,
    pub souls_lost_on_death: f64,
    /// Largest first; sources averaging under one soul are left out.
    pub souls_by_source: Vec<CurveSouls>,
}

#[derive(Serialize, JsonSchema)]
pub struct PerformanceCurve {
    pub hero: Option<String>,
    /// Averages over all players' matches, totals up to each stats snapshot (every 3 minutes up to
    /// 15:00, then every 5). Times fewer than 5% of the matches reach are left out.
    pub points: Vec<CurvePoint>,
    pub sources: Vec<Source>,
}

pub fn performance_curve(hero: Option<&str>, filters: &Filters) -> Result<PerformanceCurve> {
    let cat = catalog()?;
    let (hero_query, hero) = hero_filter(&cat, hero)?;
    let (rows, source) = api::performance_curve(&format!("{hero_query}{}", filters.query()))?;
    let most = rows.iter().map(|p| p.samples).max().unwrap_or_default();
    let points = rows
        .iter()
        .filter(|p| p.samples * 20 >= most)
        .map(|p| {
            let mut souls_by_source: Vec<CurveSouls> = [
                (G::KEPlayers, p.gold_player_avg + p.gold_player_orbs_avg),
                (
                    G::KELaneCreeps,
                    p.gold_lane_creep_avg + p.gold_lane_creep_orbs_avg,
                ),
                (
                    G::KENeutrals,
                    p.gold_neutral_creep_avg + p.gold_neutral_creep_orbs_avg,
                ),
                (G::KEBosses, p.gold_boss_avg + p.gold_boss_orb_avg),
                (G::KETreasure, p.gold_treasure_avg),
                (G::KEAssists, p.gold_assists_avg),
                (G::KETeamBonus, p.gold_team_bonus_avg),
                (G::KEBreakable, p.gold_breakable_avg),
                (G::KEAbilityAssassinate, p.gold_ability_assassinate_avg),
                (G::KEItemTrophyCollector, p.gold_item_trophy_collector_avg),
                (G::KEItemCultistSacrifice, p.gold_item_cultist_sacrifice_avg),
                (G::KEItemGooseEgg, p.gold_item_goose_egg_avg),
            ]
            .into_iter()
            .filter(|(_, souls)| *souls >= 1.0)
            .map(|(g, souls)| CurveSouls {
                source: gold_source_name(g),
                souls: round(souls, 0),
            })
            .collect();
            souls_by_source.sort_by(|a, b| b.souls.total_cmp(&a.souls));
            CurvePoint {
                at: mmss(u64::try_from(p.game_time).unwrap_or_default()),
                samples: p.samples,
                net_worth: round(p.net_worth_avg, 0),
                net_worth_std: round(p.net_worth_std, 0),
                kills: round(p.kills_avg, 1),
                deaths: round(p.deaths_avg, 1),
                assists: round(p.assists_avg, 1),
                last_hits: round(p.creep_kills_avg, 1),
                denies: round(p.denies_avg, 1),
                hero_damage: round(p.player_damage_avg, 0),
                healing: round(p.player_healing_avg, 0),
                souls_lost_on_death: round(p.gold_death_loss_avg, 0),
                souls_by_source,
            }
        })
        .collect();
    Ok(PerformanceCurve {
        hero,
        points,
        sources: sources(source, &cat),
    })
}

/// The metrics `stat_percentiles` gives when none are asked for.
const DEFAULT_METRICS: [Metric; 7] = [
    Metric::NetWorthPerMin,
    Metric::Kills,
    Metric::Deaths,
    Metric::Assists,
    Metric::LastHits,
    Metric::PlayerDamagePerMin,
    Metric::Accuracy,
];

/// What a metric's values are; everything is per match.
const fn metric_unit(m: Metric) -> &'static str {
    match m {
        Metric::Kills
        | Metric::Deaths
        | Metric::Assists
        | Metric::KillsPlusAssists
        | Metric::Denies
        | Metric::LastHits => "count",
        Metric::NetWorth => "souls",
        Metric::NetWorthPerMin => "souls per minute",
        Metric::CritShotRate => "share of hits that were crits (headshots), 0..1",
        Metric::Accuracy => "share of shots that hit, 0..1",
        Metric::Kd => "kills per death",
        Metric::Kda => "kills and assists per death",
        Metric::PlayerDamage => "damage to enemy heroes",
        Metric::PlayerDamagePerHealth => "damage to enemy heroes per point of own health",
        Metric::PlayerDamagePerMin => "damage to enemy heroes per minute",
        Metric::PlayerDamageTakenPerMin => "damage taken from heroes per minute",
        Metric::NeutralDamage => "damage to jungle neutrals",
        Metric::NeutralDamagePerMin => "damage to jungle neutrals per minute",
        Metric::BossDamage => "damage to objectives",
        Metric::BossDamagePerMin => "damage to objectives per minute",
        Metric::SelfHealing => "health healed on self",
        Metric::PlayerHealing => "health healed on heroes",
        Metric::Healing => "health healed",
        Metric::SelfHealingPerMin => "health healed on self per minute",
        Metric::PlayerHealingPerMin => "health healed on heroes per minute",
        Metric::HealingPerMin => "health healed per minute",
        Metric::TeammateHealing => "health healed on teammates",
        Metric::TeammateBarriering => "barrier given to teammates",
        Metric::HealPrevented => "enemy healing prevented",
        Metric::PermanentBuffs => "permanent buffs picked up",
        Metric::PermanentBuffsPerMin => "permanent buffs picked up per minute",
        Metric::FirstPermanentBuffTimeS => "game time of the first permanent buff, seconds",
    }
}

/// The value below which that share of single-match values lies, e.g. `p25` = 25th percentile.
#[derive(Serialize, JsonSchema)]
pub struct Percentiles {
    pub p1: Option<f64>,
    pub p5: Option<f64>,
    pub p10: Option<f64>,
    pub p25: Option<f64>,
    pub p50: Option<f64>,
    pub p75: Option<f64>,
    pub p90: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
pub struct MetricRow {
    pub metric: Metric,
    pub unit: &'static str,
    pub avg: Option<f64>,
    /// `null` throughout when no match in the sample has the metric.
    pub percentiles: Percentiles,
}

#[derive(Serialize, JsonSchema)]
pub struct StatPercentiles {
    pub hero: Option<String>,
    /// Each player's row in each match is one value, so the percentiles rank a single match, not
    /// a player's average (which spreads much narrower). The API gives no match count.
    pub metrics: Vec<MetricRow>,
    pub sources: Vec<Source>,
}

pub fn stat_percentiles(
    hero: Option<&str>,
    metrics: &[Metric],
    filters: &Filters,
) -> Result<StatPercentiles> {
    let cat = catalog()?;
    let (mut query, hero) = hero_filter(&cat, hero)?;
    let metrics = if metrics.is_empty() {
        &DEFAULT_METRICS[..]
    } else {
        metrics
    };
    // The API leaves the buff metrics out unless asked.
    if metrics.iter().any(|m| {
        matches!(
            m,
            Metric::PermanentBuffs | Metric::PermanentBuffsPerMin | Metric::FirstPermanentBuffTimeS
        )
    }) {
        query.push_str("&include_buff_metrics=true");
    }
    let (all, source) = api::stat_metrics(&format!("{query}{}", filters.query()))?;
    let metrics = metrics
        .iter()
        .map(|&m| {
            let v = all.get(&m);
            let at = |f: fn(&MetricValues) -> Option<f64>| v.and_then(f).map(|x| round(x, 3));
            MetricRow {
                metric: m,
                unit: metric_unit(m),
                avg: at(|v| v.avg),
                percentiles: Percentiles {
                    p1: at(|v| v.percentile1),
                    p5: at(|v| v.percentile5),
                    p10: at(|v| v.percentile10),
                    p25: at(|v| v.percentile25),
                    p50: at(|v| v.percentile50),
                    p75: at(|v| v.percentile75),
                    p90: at(|v| v.percentile90),
                    p95: at(|v| v.percentile95),
                    p99: at(|v| v.percentile99),
                },
            }
        })
        .collect();
    Ok(StatPercentiles {
        hero,
        metrics,
        sources: sources(source, &cat),
    })
}
