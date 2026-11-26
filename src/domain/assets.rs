//! The game catalog from the Deadlock API's asset endpoints (built from the game files): heroes,
//! items (abilities, shop items, weapons), rank names and lanes, all of the newest game version
//! the API has processed. Parsed once per version and kept in memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api;
use crate::http::Source;
use crate::problem::Result;
use crate::sources::api as source;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Ability,
    Weapon,
    /// A shop item.
    Upgrade,
}

pub struct Item {
    pub id: u32,
    pub name: String,
    pub kind: ItemKind,
    /// Shop category of an upgrade: `weapon`, `spirit` or `vitality`.
    pub slot: Option<String>,
    pub hero: Option<u32>,
    pub tier: Option<i32>,
    pub cost: Option<i64>,
}

impl From<api::Item> for Item {
    fn from(item: api::Item) -> Self {
        let id32 = |id: i64| u32::try_from(id).unwrap_or_default();
        match item {
            api::Item::Ability { id, name, hero, .. } => Self {
                id: id32(id),
                name,
                kind: ItemKind::Ability,
                slot: None,
                hero: hero.map(id32),
                tier: None,
                cost: None,
            },
            api::Item::Weapon { id, name, hero, .. } => Self {
                id: id32(id),
                name,
                kind: ItemKind::Weapon,
                slot: None,
                hero: hero.map(id32),
                tier: None,
                cost: None,
            },
            api::Item::Upgrade {
                id,
                name,
                hero,
                item_slot_type,
                item_tier,
                cost,
                ..
            } => Self {
                id: id32(id),
                name,
                kind: ItemKind::Upgrade,
                slot: Some(item_slot_type.to_string()),
                hero: hero.map(id32),
                tier: Some(item_tier),
                cost,
            },
        }
    }
}

/// A lane as the game names and colors it (`lane_info` in the game's generic data). Not Valve's
/// `CMsgLaneColor`: its names (3 Green, 6 Purple) are from the four-lane map before the 2025-02-25
/// update; slot 6 is now the green lane, Greenwich (<https://deadlock.wiki/Zipline>).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Lane {
    pub name: String,
    /// The lane's color in the game, `#RRGGBB`.
    pub color: Option<String>,
}

pub struct Catalog {
    /// The game version (`client_version`) the catalog describes.
    pub version: u32,
    pub heroes: HashMap<u32, String>,
    pub items: HashMap<u32, Item>,
    ranks: HashMap<i64, String>,
    /// By `assigned_lane`, the index into `lane_info`.
    lanes: Vec<(u32, Lane)>,
    pub sources: Vec<Source>,
}

impl Catalog {
    pub fn hero(&self, id: u32) -> String {
        self.heroes
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("hero {id}"))
    }

    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.get(&id)
    }

    /// `tier * 10 + subrank`, e.g. 116 = "Eternus VI"; 0 = unranked (Obscurus).
    pub fn rank(&self, badge: i64) -> String {
        const SUBRANKS: [&str; 6] = ["I", "II", "III", "IV", "V", "VI"];
        let Some(tier) = self.ranks.get(&(badge / 10)) else {
            return format!("badge {badge}");
        };
        let sub = usize::try_from(badge % 10)
            .ok()
            .and_then(|s| s.checked_sub(1))
            .and_then(|s| SUBRANKS.get(s));
        sub.map_or_else(|| tier.clone(), |s| format!("{tier} {s}"))
    }

    pub fn lane(&self, id: u32) -> Lane {
        self.lanes.iter().find(|(i, _)| *i == id).map_or_else(
            || Lane {
                name: format!("lane {id}"),
                color: None,
            },
            |(_, lane)| lane.clone(),
        )
    }

    /// The catalog's sources that came from an outdated copy, for answers to list next to their own.
    pub fn stale_sources(&self) -> impl Iterator<Item = Source> + '_ {
        self.sources.iter().filter(|s| s.stale).cloned()
    }

    /// The lanes heroes are assigned to, in the game's order.
    pub fn lanes(&self) -> &[(u32, Lane)] {
        &self.lanes
    }
}

fn hex(c: &api::Color) -> String {
    let byte = |v: i32| u8::try_from(v.clamp(0, 255)).unwrap_or_default();
    format!(
        "#{:02X}{:02X}{:02X}",
        byte(c.red),
        byte(c.green),
        byte(c.blue)
    )
}

/// Re-parsing 6.5 MB of JSON per call would be slow; the version check is a cached request. The
/// lock is held while loading, so parallel first calls wait for one load instead of each doing it
/// (a std mutex on tokio's blocking pool, never across an `.await`).
static CACHE: Mutex<Option<Arc<Catalog>>> = Mutex::new(None);

pub fn catalog() -> Result<Arc<Catalog>> {
    let (versions, version_source) = source::client_versions()?;
    let version = versions.iter().max().copied().ok_or_else(|| {
        crate::problem::Problem::new(
            crate::problem::Kind::InvalidResponse,
            "the API lists no game versions",
        )
        .with_instance(version_source.url)
    })?;
    let mut guard = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(catalog) = guard.as_ref().filter(|c| c.version == version) {
        return Ok(Arc::clone(catalog));
    }
    let catalog = Arc::new(load(version)?);
    *guard = Some(Arc::clone(&catalog));
    Ok(catalog)
}

fn load(version: u32) -> Result<Catalog> {
    let mut sources = Vec::new();
    let (heroes, s) = source::heroes(version)?;
    sources.push(s);
    let heroes = heroes
        .into_iter()
        .filter_map(|h| Some((u32::try_from(h.id).ok()?, h.name)))
        .collect();
    let (items, s) = source::items(version)?;
    sources.push(s);
    let items = items
        .into_iter()
        .map(|i| {
            let item = Item::from(i);
            (item.id, item)
        })
        .collect();
    let (ranks, s) = source::ranks(version)?;
    sources.push(s);
    let ranks = ranks.into_iter().map(|r| (r.tier, r.name)).collect();
    // Unused slots have no color; the enemy-lane entry is a marker, not a lane.
    let (data, s) = source::generic_data(version)?;
    sources.push(s);
    let lanes = data
        .lane_info
        .into_iter()
        .enumerate()
        .filter(|(_, l)| !l.is_enemy_lane && l.color.is_some())
        .filter_map(|(i, l)| {
            Some((
                u32::try_from(i).ok()?,
                Lane {
                    color: l.color.as_ref().map(hex),
                    name: l.lane_name,
                },
            ))
        })
        .collect();
    Ok(Catalog {
        version,
        heroes,
        items,
        ranks,
        lanes,
        sources,
    })
}
