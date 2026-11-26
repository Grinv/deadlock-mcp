//! The meta from the Deadlock API's analytics over all players' matches: `hero_stats`,
//! `hero_items`, `hero_counters`, `hero_synergies`, `ability_order`; plus `builds` and
//! `leaderboard`.

use std::cmp::Reverse;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::{bounded, date, mmss, plain};
use crate::domain::items::ItemKind;
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

pub fn builds(hero: &str, search: Option<&str>, limit: u32) -> Result<HeroBuilds> {
    let cat = catalog()?;
    let hero_id = cat.find_hero(hero)?;
    let mut query =
        format!("&hero_id={hero_id}&sort_by=weekly_favorites&sort_direction=desc&limit={limit}");
    if let Some(s) = search {
        let _ = write!(query, "&search_name={}", crate::http::encode(s));
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
