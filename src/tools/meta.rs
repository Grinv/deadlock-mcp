//! The meta from the Deadlock API's analytics over all players' matches: `hero_stats`,
//! `hero_items`, `hero_counters`, `hero_synergies`, `ability_order`, `lane_matchups`; plus
//! `builds` and `leaderboard`.

use std::cmp::Reverse;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::contracts::api::{
    AnalyticsAbilityOrderStats, AnalyticsHeroStats, HeroCounterStats, HeroSynergyStats,
    ItemSlotType, ItemStats, LaneMatchupStats, LaneSoulCurve, LaneSoulCurveStatsKey, MetricValues,
    PlayerPerformanceCurvePoint, PlayerStatsMetricsKey as Metric, Region,
};
use crate::contracts::valve::c_msg_match_meta_data_contents::EGoldSource as G;
use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::{clip, date, mmss, plain};
use crate::domain::items::ItemKind;
use crate::domain::terms::gold_source_name;
use crate::http::{Source, id_list};
use crate::problem::{Problem, Result};
use crate::sources::api;
use crate::tools::player::steam_names;
use crate::tools::{
    Filters, MetaFilters, answer_sources, hero_filter, id32, per, round, take, win_rate,
};

/// Win rate of a row for sorting; rows without matches sort last.
fn rate(wins: i64, matches: i64) -> f64 {
    win_rate(wins, matches).unwrap_or(0.0)
}

/// Heroes per match: a sum of the heroes' `matches` counts every match this many times.
const HEROES_PER_MATCH: i64 = 12;

/// The share of matches a hero was in, counting only the days since its first match in the
/// window, so a hero released inside the window is not divided by matches it could not be in.
fn pick_rate(matches: i64, matches_since_first_day: i64) -> f64 {
    win_rate(matches * HEROES_PER_MATCH, matches_since_first_day).unwrap_or_default()
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
    /// Share of matches the hero was in since its first match in the window, 0..1.
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

pub fn hero_stats(
    filters: &MetaFilters,
    min_matches: u32,
    sort: HeroSort,
    limit: u32,
) -> Result<HeroStats> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
    let (rows, source) = api::hero_stats(&filters.query())?;
    let sources = answer_sources([source], &cat);
    Ok(hero_stats_from(
        &cat,
        min_matches,
        sort,
        limit,
        &rows,
        sources,
    ))
}

fn hero_stats_from(
    cat: &Catalog,
    min_matches: u32,
    sort: HeroSort,
    limit: u32,
    rows: &[AnalyticsHeroStats],
    sources: Vec<Source>,
) -> HeroStats {
    let matches = rows.iter().map(|h| h.matches).sum::<i64>() / HEROES_PER_MATCH;
    let mut heroes: Vec<_> = rows
        .iter()
        .filter(|h| h.matches >= i64::from(min_matches))
        .map(|h| HeroRow {
            hero: cat.hero(id32(h.hero_id.into())),
            matches: h.matches,
            win_rate: win_rate(h.wins, h.matches),
            pick_rate: pick_rate(h.matches, h.bucket_matches_since_first_day),
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
        HeroSort::PickRate => heroes.sort_by(|a, b| b.pick_rate.total_cmp(&a.pick_rate)),
    }
    heroes.truncate(take(limit));
    HeroStats {
        matches,
        heroes,
        sources,
    }
}

#[derive(Serialize, JsonSchema)]
pub struct ItemRow {
    pub item: String,
    pub slot: Option<ItemSlotType>,
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
    filters: &MetaFilters,
    min_matches: u32,
    limit: u32,
) -> Result<HeroItems> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
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
        let _ = write!(query, "&enemy_hero_ids={}", crate::http::id_list(&enemies));
    }
    let (rows, source) = api::item_stats(&query)?;
    let sources = answer_sources([source], &cat);
    Ok(hero_items_from(
        &cat, hero_id, &enemies, limit, &rows, sources,
    ))
}

fn hero_items_from(
    cat: &Catalog,
    hero_id: u32,
    enemies: &[u32],
    limit: u32,
    rows: &[ItemStats],
    sources: Vec<Source>,
) -> HeroItems {
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
    HeroItems {
        hero: cat.hero(hero_id),
        enemy_heroes: enemies.iter().map(|&h| cat.hero(h)).collect(),
        items: items
            .into_iter()
            .take(take(limit))
            .map(|(r, i)| ItemRow {
                item: i.name.clone(),
                slot: i.slot,
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
        sources,
    }
}

/// Another hero and how matches with or against it went for the hero asked about.
#[derive(Serialize, JsonSchema)]
pub struct Pairing {
    pub hero: String,
    pub matches: i64,
    /// The asked-about hero's wins over those matches, 0..1.
    pub win_rate: Option<f64>,
}

/// `(other hero, wins, matches)`.
type PairRow = (u32, i64, i64);

/// Best win rate first; ties by hero id, so every call gives the same order.
fn by_win_rate(a: &PairRow, b: &PairRow) -> std::cmp::Ordering {
    rate(b.1, b.2)
        .total_cmp(&rate(a.1, a.2))
        .then(a.0.cmp(&b.0))
}

/// Each of `picks` with its row and its place among `rows` by win rate, 1 = the best; `None`
/// for a hero without a row (fewer matches than `min_matches`).
fn standings(rows: &[PairRow], picks: &[u32]) -> Vec<(u32, Option<(PairRow, usize)>)> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(by_win_rate);
    picks
        .iter()
        .map(|&h| {
            let found = sorted
                .iter()
                .position(|r| r.0 == h)
                .map(|i| (sorted[i], i + 1));
            (h, found)
        })
        .collect()
}

/// The hero's win rate over all its pairs, the baseline a single pairing is read against.
fn average_win_rate(rows: &[PairRow]) -> Option<f64> {
    let (wins, matches) = rows.iter().fold((0, 0), |(w, m), r| (w + r.1, m + r.2));
    win_rate(wins, matches)
}

/// Rows as the best and the worst `limit` pairings.
fn best_and_worst(cat: &Catalog, rows: Vec<PairRow>, limit: u32) -> (Vec<Pairing>, Vec<Pairing>) {
    let (best, worst) = split_best_worst(rows, take(limit));
    let pairing = |&(h, wins, matches): &PairRow| Pairing {
        hero: cat.hero(h),
        matches,
        win_rate: win_rate(wins, matches),
    };
    (
        best.iter().map(pairing).collect(),
        worst.iter().map(pairing).collect(),
    )
}

/// The `n` rows with the best win rate and the `n` with the worst, worst first. With fewer than
/// `2 * n` rows the better half is best and the rest worst, so no row is in both.
fn split_best_worst(mut rows: Vec<PairRow>, n: usize) -> (Vec<PairRow>, Vec<PairRow>) {
    rows.sort_by(by_win_rate);
    let best = n.min(rows.len().div_ceil(2));
    let worst = rows[best..].iter().rev().take(n).copied().collect();
    rows.truncate(best);
    (rows, worst)
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
    /// The hero's win rate over all its enemies here, to read each pairing against.
    pub average_win_rate: Option<f64>,
    /// The enemies asked for in `heroes`, in that order.
    pub heroes: Vec<Matchup>,
    /// Enemies the hero wins most against, best first.
    pub best_against: Vec<Pairing>,
    /// Enemies the hero loses most against, worst first.
    pub worst_against: Vec<Pairing>,
    pub sources: Vec<Source>,
}

pub fn hero_counters(
    hero: &str,
    filters: &MetaFilters,
    min_matches: u32,
    same_lane: bool,
    limit: u32,
    heroes: &[String],
) -> Result<Counters> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
    let hero_id = cat.find_hero(hero)?;
    let picks = heroes
        .iter()
        .map(|h| cat.find_hero(h))
        .collect::<Result<Vec<u32>>>()?;
    let (rows, source) = api::hero_counter_stats(&pair_query(&filters, min_matches, same_lane))?;
    let sources = answer_sources([source], &cat);
    Ok(hero_counters_from(
        &cat, hero_id, same_lane, limit, &picks, &rows, sources,
    ))
}

fn hero_counters_from(
    cat: &Catalog,
    hero_id: u32,
    same_lane: bool,
    limit: u32,
    picks: &[u32],
    rows: &[HeroCounterStats],
    sources: Vec<Source>,
) -> Counters {
    let rows: Vec<PairRow> = rows
        .iter()
        .filter(|r| id32(r.hero_id.into()) == hero_id)
        .map(|r| (id32(r.enemy_hero_id.into()), r.wins, r.matches_played))
        .collect();
    let of = rows.len();
    let heroes = standings(&rows, picks)
        .into_iter()
        .map(|(h, found)| Matchup {
            hero: cat.hero(h),
            matches: found.map_or(0, |(r, _)| r.2),
            win_rate: found.and_then(|(r, _)| win_rate(r.1, r.2)),
            hardest_place: found.map(|(_, best)| of + 1 - best),
            of,
        })
        .collect();
    let average_win_rate = average_win_rate(&rows);
    let (best_against, worst_against) = best_and_worst(cat, rows, limit);
    Counters {
        hero: cat.hero(hero_id),
        same_lane,
        average_win_rate,
        heroes,
        best_against,
        worst_against,
        sources,
    }
}

/// An enemy asked for, and how hard it is for the hero next to all the others.
#[derive(Serialize, JsonSchema)]
pub struct Matchup {
    pub hero: String,
    pub matches: i64,
    /// The asked-about hero's wins over those matches, 0..1.
    pub win_rate: Option<f64>,
    /// 1 = the enemy the hero loses to most, `of` = the one it beats most; `None` when the pair has
    /// fewer matches than `min_matches`.
    pub hardest_place: Option<usize>,
    /// How many enemies have enough matches to be ranked.
    pub of: usize,
}

/// A teammate asked for, and how well the hero does with it next to all the others.
#[derive(Serialize, JsonSchema)]
pub struct Partnership {
    pub hero: String,
    pub matches: i64,
    /// The asked-about hero's wins over those matches, 0..1.
    pub win_rate: Option<f64>,
    /// 1 = the teammate the hero wins most with, `of` = the one it wins least with; `None` when
    /// the pair has fewer matches than `min_matches`.
    pub best_place: Option<usize>,
    /// How many teammates have enough matches to be ranked.
    pub of: usize,
}

#[derive(Serialize, JsonSchema)]
pub struct Synergies {
    pub hero: String,
    /// Only teammates that shared the hero's lane.
    pub same_lane: bool,
    /// The hero's win rate over all its teammates here, to read each pairing against.
    pub average_win_rate: Option<f64>,
    /// The teammates asked for in `heroes`, in that order.
    pub heroes: Vec<Partnership>,
    /// Teammates the hero wins most with, best first.
    pub best_with: Vec<Pairing>,
    /// Teammates the hero wins least with, worst first.
    pub worst_with: Vec<Pairing>,
    pub sources: Vec<Source>,
}

pub fn hero_synergies(
    hero: &str,
    filters: &MetaFilters,
    min_matches: u32,
    same_lane: bool,
    limit: u32,
    heroes: &[String],
) -> Result<Synergies> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
    let hero_id = cat.find_hero(hero)?;
    let picks = heroes
        .iter()
        .map(|h| cat.find_hero(h))
        .collect::<Result<Vec<u32>>>()?;
    let (rows, source) = api::hero_synergy_stats(&pair_query(&filters, min_matches, same_lane))?;
    let sources = answer_sources([source], &cat);
    Ok(hero_synergies_from(
        &cat, hero_id, same_lane, limit, &picks, &rows, sources,
    ))
}

fn hero_synergies_from(
    cat: &Catalog,
    hero_id: u32,
    same_lane: bool,
    limit: u32,
    picks: &[u32],
    rows: &[HeroSynergyStats],
    sources: Vec<Source>,
) -> Synergies {
    // Each pair comes once, in either order.
    let rows: Vec<PairRow> = rows
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
    let of = rows.len();
    let heroes = standings(&rows, picks)
        .into_iter()
        .map(|(h, found)| Partnership {
            hero: cat.hero(h),
            matches: found.map_or(0, |(r, _)| r.2),
            win_rate: found.and_then(|(r, _)| win_rate(r.1, r.2)),
            best_place: found.map(|(_, best)| best),
            of,
        })
        .collect();
    let average_win_rate = average_win_rate(&rows);
    let (best_with, worst_with) = best_and_worst(cat, rows, limit);
    Synergies {
        hero: cat.hero(hero_id),
        same_lane,
        average_win_rate,
        heroes,
        best_with,
        worst_with,
        sources,
    }
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
    filters: &MetaFilters,
    min_matches: u32,
    limit: u32,
) -> Result<AbilityOrders> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
    let hero_id = cat.find_hero(hero)?;
    let query = format!("&min_matches={min_matches}{}", filters.query());
    let (rows, source) = api::ability_order_stats(hero_id, &query)?;
    let sources = answer_sources([source], &cat);
    Ok(ability_order_from(&cat, hero_id, limit, rows, sources))
}

fn ability_order_from(
    cat: &Catalog,
    hero_id: u32,
    limit: u32,
    mut rows: Vec<AnalyticsAbilityOrderStats>,
    sources: Vec<Source>,
) -> AbilityOrders {
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
    AbilityOrders {
        hero: cat.hero(hero_id),
        most_common,
        best,
        sources,
    }
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

/// Build descriptions are cut here.
const MAX_DESCRIPTION: usize = 300;

/// A build author's description as plain text, cut at `MAX_DESCRIPTION`; `None` when blank.
fn build_description(d: &str) -> Option<String> {
    Some(clip(plain(d).trim(), MAX_DESCRIPTION)).filter(|d| !d.is_empty())
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
                description: h.description.as_deref().and_then(build_description),
            }
        })
        .collect();
    Ok(HeroBuilds {
        hero: cat.hero(hero_id),
        builds,
        sources: answer_sources(std::iter::once(source).chain(names_source), &cat),
    })
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
    pub region: Region,
    pub hero: Option<String>,
    pub players: Vec<LeaderboardRow>,
    pub sources: Vec<Source>,
}

pub fn leaderboard(region: Region, hero: Option<&str>, limit: u32) -> Result<Leaderboard> {
    let cat = catalog()?;
    let hero_id = hero.map(|h| cat.find_hero(h)).transpose()?;
    let (board, source) = api::leaderboard(region, hero_id)?;
    Ok(Leaderboard {
        region,
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
        sources: answer_sources([source], &cat),
    })
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

pub fn performance_curve(hero: Option<&str>, filters: &MetaFilters) -> Result<PerformanceCurve> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
    let (hero_query, hero) = hero_filter(&cat, hero)?;
    let (rows, source) = api::performance_curve(&format!("{hero_query}{}", filters.query()))?;
    Ok(performance_curve_from(
        hero,
        &rows,
        answer_sources([source], &cat),
    ))
}

fn performance_curve_from(
    hero: Option<String>,
    rows: &[PlayerPerformanceCurvePoint],
    sources: Vec<Source>,
) -> PerformanceCurve {
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
    PerformanceCurve {
        hero,
        points,
        sources,
    }
}

#[derive(Serialize, JsonSchema)]
pub struct LanePoint {
    /// Game time of the stats snapshot, `m:ss`; `match_lanes` uses the same ones.
    pub at: String,
    /// Matchups still running at this snapshot. Fewer late in the match, as shorter matches end:
    /// late points rest on fewer matchups.
    pub matches: i64,
    /// Average souls (net worth) the duo is ahead of the enemy duo by; negative = behind.
    pub soul_lead: f64,
    /// Standard deviation of the lead between single matchups, not the uncertainty of the
    /// average: about two thirds of matchups are within it of `soul_lead`.
    pub soul_lead_spread: f64,
    /// Average damage to heroes the duo has dealt by then minus the enemy duo's; negative = less.
    pub damage_lead: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
pub struct LaneMatchups {
    /// The lane duo.
    pub heroes: Vec<String>,
    /// The duo they laned against; empty = against any duo.
    pub enemy_heroes: Vec<String>,
    /// Lanes where these duos faced each other, lanes merged; 0 when fewer than `min_matches`,
    /// and then `points` is empty too. From the API's lane matchup stats, like `win_rate`, so it
    /// can differ slightly from the curve's `matches` at 3:00.
    pub matches: i64,
    /// The duo's wins over `matches` (the whole match, not just the lane), 0..1.
    pub win_rate: Option<f64>,
    /// The duo's average lead at each stats snapshot (every 3 minutes up to 15:00, then every 5).
    pub points: Vec<LanePoint>,
    pub sources: Vec<Source>,
}

pub fn lane_matchups(
    heroes: &[String],
    enemy_heroes: &[String],
    filters: &MetaFilters,
    min_matches: u32,
) -> Result<LaneMatchups> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
    let find = |names: &[String]| {
        names
            .iter()
            .map(|h| cat.find_hero(h))
            .collect::<Result<Vec<u32>>>()
    };
    let (duo, enemies) = (find(heroes)?, find(enemy_heroes)?);
    let query = format!(
        "{}&min_matches={min_matches}{}",
        lane_query(&duo, &enemies)?,
        filters.query()
    );
    let (curves, curve_source) = api::lane_soul_curve(&query)?;
    let (records, record_source) = api::lane_matchup_stats(&query)?;
    let sources = answer_sources([curve_source, record_source], &cat);
    Ok(lane_matchups_from(
        &cat, &duo, &enemies, &curves, &records, sources,
    ))
}

/// The API's filter for a duo against a duo, or against any duo without `enemies`; lanes merged.
fn lane_query(duo: &[u32], enemies: &[u32]) -> Result<String> {
    // With one id the API takes any duo that includes it, so a repeated hero would widen the query.
    let pair = |ids: &[u32]| ids.len() == 2 && ids[0] != ids[1];
    if !pair(duo) {
        return Err(Problem::invalid_argument(
            "heroes: name the two different heroes of the lane duo",
        ));
    }
    // Sorted, so the same matchup in another order is the same query and the same cache entry.
    let sorted = |ids: &[u32]| {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        id_list(&ids)
    };
    let duo = sorted(duo);
    match enemies {
        [] => Ok(format!("&hero_ids={duo}&group_by=hero_ids")),
        _ if pair(enemies) => Ok(format!(
            "&hero_ids={duo}&enemy_hero_ids={}&group_by=hero_ids,enemy_hero_ids",
            sorted(enemies)
        )),
        _ => Err(Problem::invalid_argument(
            "enemy_heroes: name both (different) heroes of the enemy lane duo, or none for the \
             duo against any duo",
        )),
    }
}

fn lane_matchups_from(
    cat: &Catalog,
    duo: &[u32],
    enemies: &[u32],
    curves: &[LaneSoulCurve],
    records: &[LaneMatchupStats],
    sources: Vec<Source>,
) -> LaneMatchups {
    // The API answers with ascending id pairs.
    let ids = |heroes: &[u32]| {
        let mut ids: Vec<i64> = heroes.iter().map(|&h| i64::from(h)).collect();
        ids.sort_unstable();
        ids
    };
    let (duo_ids, enemy_ids) = (ids(duo), ids(enemies));
    let record = records
        .iter()
        .find(|r| r.hero_ids == duo_ids && r.enemy_hero_ids == enemy_ids);
    let points = curves
        .iter()
        .find(|c| c.hero_ids == duo_ids && c.enemy_hero_ids == enemy_ids)
        .map_or_default(|c| {
            let damage = c.stats.get(&LaneSoulCurveStatsKey::PlayerDamage);
            c.sample_times_s
                .iter()
                .zip(&c.sample_matches)
                .zip(c.net_worth_diff.iter().zip(&c.net_worth_diff_std))
                .enumerate()
                .filter(|(_, ((_, matches), _))| **matches > 0)
                .map(|(i, ((&t, &matches), (&lead, &spread)))| LanePoint {
                    at: mmss(u64::try_from(t).unwrap_or_default()),
                    matches,
                    soul_lead: round(lead, 0),
                    soul_lead_spread: round(spread, 0),
                    damage_lead: damage.and_then(|d| d.diff.get(i)).map(|&d| round(d, 0)),
                })
                .collect()
        });
    let name = |heroes: &[u32]| heroes.iter().map(|&h| cat.hero(h)).collect();
    LaneMatchups {
        heroes: name(duo),
        enemy_heroes: name(enemies),
        matches: record.map_or(0, |r| r.matches_played),
        win_rate: record.and_then(|r| win_rate(r.wins, r.matches_played)),
        points,
        sources,
    }
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
    filters: &MetaFilters,
) -> Result<StatPercentiles> {
    let cat = catalog()?;
    let filters = filters.to_filters(&cat)?;
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
        sources: answer_sources([source], &cat),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::items::Item;

    fn fixture<T: serde::de::DeserializeOwned>(json: &str) -> Vec<T> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn hero_stats_filter_sort_and_cut() {
        let mut rows: Vec<AnalyticsHeroStats> = fixture(include_str!(
            "../../contracts/fixtures/analytics_hero_stats.json"
        ));
        let set = |r: &mut AnalyticsHeroStats, hero, matches, wins, kills, net_worth| {
            (r.hero_id, r.matches, r.wins) = (hero, matches, wins);
            (r.total_kills, r.total_deaths, r.total_assists) = (kills, matches / 2, 0);
            (r.total_net_worth, r.bucket_matches_since_first_day) = (net_worth, 12_000);
        };
        set(&mut rows[0], 1, 120, 60, 240, 1_200_049);
        set(&mut rows[1], 2, 60, 45, 60, 30_000);
        let mut rare = rows[1].clone();
        set(&mut rare, 3, 10, 10, 0, 0);
        rows.push(rare);
        let cat = Catalog::test(&[(1, "H1"), (2, "H2"), (3, "H3")]);

        // 190 hero-matches are 15 matches; H3 has fewer than 50 and is left out.
        let by_wins = hero_stats_from(&cat, 50, HeroSort::WinRate, 5, &rows, Vec::new());
        assert_eq!(
            serde_json::to_value(&by_wins).unwrap(),
            serde_json::json!({
                "matches": 15,
                "heroes": [
                    {"hero": "H2", "matches": 60, "win_rate": 0.75, "pick_rate": 0.06,
                     "kills_per_match": 1.0, "deaths_per_match": 0.5, "assists_per_match": 0.0,
                     "souls_per_match": 500.0},
                    {"hero": "H1", "matches": 120, "win_rate": 0.5, "pick_rate": 0.12,
                     "kills_per_match": 2.0, "deaths_per_match": 0.5, "assists_per_match": 0.0,
                     "souls_per_match": 10000.0},
                ],
                "sources": [],
            })
        );
        let by_picks = hero_stats_from(&cat, 50, HeroSort::PickRate, 1, &rows, Vec::new());
        let heroes: Vec<&str> = by_picks.heroes.iter().map(|h| h.hero.as_str()).collect();
        assert_eq!(heroes, ["H1"]);
    }

    #[test]
    fn hero_items_are_shop_items_by_win_rate() {
        let base: Vec<ItemStats> =
            fixture(include_str!("../../contracts/fixtures/item_stats.json"));
        let row = |item_id, wins, matches, buy| {
            let mut r = base[0].clone();
            (r.item_id, r.wins, r.matches, r.avg_buy_time_s) = (item_id, wins, matches, buy);
            r
        };
        let rows = [
            row(12, 1, 4, -5.0),
            row(11, 4, 4, 60.0),
            row(10, 3, 4, 125.0),
            row(99, 4, 4, 60.0),
        ];
        let cat = Catalog::test(&[(1, "H1"), (2, "H2")]).with_items(vec![
            Item::test(10, "Up A", ItemKind::Upgrade),
            Item::test(11, "Ability", ItemKind::Ability),
            Item::test(12, "Up B", ItemKind::Upgrade),
        ]);
        let items = hero_items_from(&cat, 1, &[2], 5, &rows, Vec::new());
        // The ability and the item the catalog doesn't know are left out; a negative time is 0:00.
        assert_eq!(
            serde_json::to_value(&items).unwrap(),
            serde_json::json!({
                "hero": "H1",
                "enemy_heroes": ["H2"],
                "items": [
                    {"item": "Up A", "slot": null, "tier": null, "cost": null, "matches": 4,
                     "win_rate": 0.75, "avg_buy_time": "2:05"},
                    {"item": "Up B", "slot": null, "tier": null, "cost": null, "matches": 4,
                     "win_rate": 0.25, "avg_buy_time": "0:00"},
                ],
                "sources": [],
            })
        );
    }

    #[test]
    fn hero_counters_read_only_the_heros_rows() {
        let base: Vec<HeroCounterStats> = fixture(include_str!(
            "../../contracts/fixtures/hero_counter_stats.json"
        ));
        let row = |hero, enemy, wins| {
            let mut r = base[0].clone();
            (r.hero_id, r.enemy_hero_id, r.wins, r.matches_played) = (hero, enemy, wins, 10);
            r
        };
        // The last row is hero 2's view of the same pairing and must not count for hero 1.
        let rows = [row(1, 2, 6), row(1, 3, 3), row(2, 1, 9)];
        let cat = Catalog::test(&[(1, "H1"), (2, "H2"), (3, "H3")]);
        // H3 is the enemy H1 loses to most; H4 has no pair with enough matches.
        let c = hero_counters_from(&cat, 1, true, 1, &[3, 2, 4], &rows, Vec::new());
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({
                "hero": "H1",
                "same_lane": true,
                "average_win_rate": 0.45,
                "heroes": [
                    {"hero": "H3", "matches": 10, "win_rate": 0.3, "hardest_place": 1, "of": 2},
                    {"hero": "H2", "matches": 10, "win_rate": 0.6, "hardest_place": 2, "of": 2},
                    {"hero": "hero 4", "matches": 0, "win_rate": null, "hardest_place": null, "of": 2},
                ],
                "best_against": [{"hero": "H2", "matches": 10, "win_rate": 0.6}],
                "worst_against": [{"hero": "H3", "matches": 10, "win_rate": 0.3}],
                "sources": [],
            })
        );
    }

    #[test]
    fn ability_orders_most_common_and_best() {
        let base: Vec<AnalyticsAbilityOrderStats> = fixture(include_str!(
            "../../contracts/fixtures/ability_order_stats.json"
        ));
        let row = |abilities: Vec<i64>, wins, matches| {
            let mut r = base[0].clone();
            (r.abilities, r.wins, r.matches) = (abilities, wins, matches);
            r
        };
        let rows = vec![row(vec![11], 40, 50), row(vec![10, 99], 40, 100)];
        let cat = Catalog::test(&[(1, "H1")]).with_items(vec![
            Item::test(10, "Ab A", ItemKind::Ability),
            Item::test(11, "Ab B", ItemKind::Ability),
        ]);
        let o = ability_order_from(&cat, 1, 1, rows, Vec::new());
        // Most played first, then best win rate; an id the catalog lacks keeps its number.
        assert_eq!(
            serde_json::to_value(&o).unwrap(),
            serde_json::json!({
                "hero": "H1",
                "most_common": [{"abilities": ["Ab A", "ability 99"], "matches": 100, "win_rate": 0.4}],
                "best": [{"abilities": ["Ab B"], "matches": 50, "win_rate": 0.8}],
                "sources": [],
            })
        );
    }

    /// A hero in 10 of the 100 matches since its release has a pick rate of 0.1, whatever came
    /// before it in the window.
    #[test]
    fn pick_rate_counts_from_the_first_day() {
        assert!((pick_rate(10, 100 * HEROES_PER_MATCH) - 0.1).abs() < 1e-9);
        assert!(pick_rate(0, 0).abs() < f64::EPSILON);
    }

    #[test]
    fn synergies_take_either_side_of_a_pair() {
        let mut rows: Vec<HeroSynergyStats> = serde_json::from_str(include_str!(
            "../../contracts/fixtures/hero_synergy_stats.json"
        ))
        .unwrap();
        // The fixture's pairs (1, 2) and (1, 3); turn the second around and add one without hero 1.
        (rows[1].hero_id1, rows[1].hero_id2) = (3, 1);
        let mut other = rows[0].clone();
        (other.hero_id1, other.hero_id2) = (2, 3);
        rows.push(other);
        let cat = Catalog::test(&[(1, "H1"), (2, "H2"), (3, "H3")]);
        let s = hero_synergies_from(&cat, 1, false, 1, &[3], &rows, Vec::new());
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::json!({
                "hero": "H1",
                "same_lane": false,
                // (36561 + 30166) / (67595 + 63618)
                "average_win_rate": 0.509,
                "heroes": [{"hero": "H3", "matches": 63618, "win_rate": 0.474, "best_place": 2, "of": 2}],
                "best_with": [{"hero": "H2", "matches": 67595, "win_rate": 0.541}],
                "worst_with": [{"hero": "H3", "matches": 63618, "win_rate": 0.474}],
                "sources": [],
            })
        );
    }

    #[test]
    fn performance_curve_leaves_out_rare_times() {
        let mut rows: Vec<PlayerPerformanceCurvePoint> = serde_json::from_str(include_str!(
            "../../contracts/fixtures/performance_curve.json"
        ))
        .unwrap();
        let at = |rows: &[PlayerPerformanceCurvePoint]| {
            performance_curve_from(None, rows, Vec::new())
                .points
                .into_iter()
                .map(|p| p.at)
                .collect::<Vec<_>>()
        };
        // 5% of the most samples (39769) is 1988.45.
        rows[1].samples = 1989;
        assert_eq!(at(&rows), ["3:00", "6:00"]);
        rows[1].samples = 1988;
        assert_eq!(at(&rows), ["3:00"]);

        let curve = performance_curve_from(Some("H1".to_owned()), &rows, Vec::new());
        assert_eq!(curve.hero.as_deref(), Some("H1"));
        // Orbs added to their source, under one soul left out, largest first.
        assert_eq!(
            serde_json::to_value(&curve.points[0].souls_by_source).unwrap(),
            serde_json::json!([
                {"source": "lane troopers", "souls": 1008.0},
                {"source": "hero kills", "souls": 118.0},
                {"source": "jungle (neutrals, incl. Sinner's Sacrifice)", "souls": 50.0},
                {"source": "assists", "souls": 35.0},
                {"source": "breakables (crates)", "souls": 12.0},
            ])
        );
    }

    fn lane_fixtures() -> (Vec<LaneSoulCurve>, Vec<LaneMatchupStats>) {
        (
            fixture(include_str!(
                "../../contracts/fixtures/lane_soul_curve.json"
            )),
            fixture(include_str!(
                "../../contracts/fixtures/lane_matchup_stats.json"
            )),
        )
    }

    fn lane_catalog() -> Catalog {
        Catalog::test(&[
            (11, "Dynamo"),
            (76, "Graves"),
            (84, "Rat King"),
            (85, "Solomon"),
        ])
    }

    #[test]
    fn lane_matchups_map_the_curve_to_points() {
        let (mut curves, records) = lane_fixtures();
        let cat = lane_catalog();
        // Enemies in another order than the API's ascending pair still find the row.
        let lanes = lane_matchups_from(&cat, &[76, 11], &[85, 84], &curves, &records, Vec::new());
        assert_eq!(
            serde_json::to_value(&lanes).unwrap(),
            serde_json::json!({
                "heroes": ["Graves", "Dynamo"],
                "enemy_heroes": ["Solomon", "Rat King"],
                "matches": 124,
                "win_rate": 0.516,
                "points": [
                    {"at": "3:00", "matches": 124, "soul_lead": -38.0, "soul_lead_spread": 599.0,
                     "damage_lead": -605.0},
                    {"at": "6:00", "matches": 124, "soul_lead": -75.0, "soul_lead_spread": 1181.0,
                     "damage_lead": -1936.0},
                    {"at": "9:00", "matches": 124, "soul_lead": -656.0,
                     "soul_lead_spread": 1872.0, "damage_lead": -3833.0},
                    {"at": "12:00", "matches": 124, "soul_lead": -1024.0,
                     "soul_lead_spread": 2745.0, "damage_lead": -5067.0},
                    {"at": "15:00", "matches": 124, "soul_lead": -951.0,
                     "soul_lead_spread": 3658.0, "damage_lead": -6130.0},
                ],
                "sources": [],
            })
        );
        // A snapshot no matchup reached is left out; a missing damage curve leaves only its lead
        // empty.
        curves[0].sample_matches[4] = 0;
        curves[0].stats.clear();
        let lanes = lane_matchups_from(&cat, &[11, 76], &[84, 85], &curves, &records, Vec::new());
        assert_eq!(lanes.points.len(), 4);
        assert_eq!(lanes.points[3].at, "12:00");
        assert_eq!(lanes.points[3].damage_lead, None);
    }

    #[test]
    fn lane_matchups_without_rows_are_empty() {
        let (curves, records) = lane_fixtures();
        let cat = lane_catalog();
        // Rows of another matchup (here: the duo against anyone) don't count.
        let lanes = lane_matchups_from(&cat, &[11, 76], &[], &curves, &records, Vec::new());
        assert_eq!(
            serde_json::to_value(&lanes).unwrap(),
            serde_json::json!({
                "heroes": ["Dynamo", "Graves"],
                "enemy_heroes": [],
                "matches": 0,
                "win_rate": null,
                "points": [],
                "sources": [],
            })
        );
        let lanes = lane_matchups_from(&cat, &[11, 76], &[84, 85], &[], &[], Vec::new());
        assert_eq!((lanes.matches, lanes.points.len()), (0, 0));
    }

    #[test]
    fn lane_query_takes_two_enemies_or_none() {
        let query =
            |duo: &[u32], enemies: &[u32]| lane_query(duo, enemies).map_err(|p| p.message());
        assert_eq!(
            query(&[11, 76], &[84, 85]).as_deref(),
            Ok("&hero_ids=11,76&enemy_hero_ids=84,85&group_by=hero_ids,enemy_hero_ids")
        );
        // Order doesn't matter: one query, one cache entry.
        assert_eq!(query(&[76, 11], &[85, 84]), query(&[11, 76], &[84, 85]));
        assert_eq!(
            query(&[11, 76], &[]).as_deref(),
            Ok("&hero_ids=11,76&group_by=hero_ids")
        );
        let one = query(&[11, 76], &[84]).unwrap_err();
        assert!(one.contains("enemy_heroes: name both"), "{one}");
        assert!(query(&[11, 76], &[84, 84]).is_err());
        assert!(query(&[11, 11], &[]).unwrap_err().contains("heroes:"));
    }

    #[test]
    fn build_descriptions_are_cut_and_blank_ones_dropped() {
        let long = format!(" {}\u{200b}", "a".repeat(MAX_DESCRIPTION + 1));
        assert_eq!(
            build_description(&long),
            Some(format!("{}…", "a".repeat(MAX_DESCRIPTION)))
        );
        assert_eq!(build_description(" short "), Some("short".to_owned()));
        assert_eq!(build_description(" \u{200b} "), None);
    }

    #[test]
    fn best_and_worst_never_share_a_row() {
        // (hero, wins, matches): win rates 0.9, 0.7, 0.5, 0.3, 0.1.
        let rows = vec![(3, 5, 10), (1, 9, 10), (5, 1, 10), (2, 7, 10), (4, 3, 10)];
        assert_eq!(
            split_best_worst(rows.clone(), 2),
            (vec![(1, 9, 10), (2, 7, 10)], vec![(5, 1, 10), (4, 3, 10)])
        );
        // Fewer than 2 * n rows: split in halves instead of listing a hero twice.
        assert_eq!(
            split_best_worst(rows, 10),
            (
                vec![(1, 9, 10), (2, 7, 10), (3, 5, 10)],
                vec![(5, 1, 10), (4, 3, 10)]
            )
        );
    }
}
