//! The game catalog from the Deadlock API's asset endpoints (built from the game files): heroes,
//! items (abilities, shop items, weapons), rank names and lanes, all of the newest game version
//! the API has processed. Parsed once per version and kept in memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api;
use crate::domain::format::strip_tags;
use crate::domain::items::Item;
use crate::http::Source;
use crate::problem::{Problem, Result};
use crate::sources::api as source;

/// A lane as the game names and colors it (`lane_info` in the game's generic data). Not Valve's
/// `CMsgLaneColor`: its names (3 Green, 6 Purple) are from the four-lane map before the 2025-02-25
/// update; slot 6 is now the green lane, Greenwich (<https://deadlock.wiki/Zipline>).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Lane {
    pub name: String,
    /// The lane's color in the game, `#RRGGBB`.
    pub color: Option<String>,
}

/// What the game says about a hero, as plain text.
pub struct HeroInfo {
    pub role: Option<String>,
    pub playstyle: Option<String>,
    pub lore: Option<String>,
    /// Playable in normal matches (not a test or unreleased hero).
    pub selectable: bool,
}

pub struct Catalog {
    /// The game version (`client_version`) the catalog describes.
    pub version: u32,
    pub heroes: HashMap<u32, String>,
    pub hero_info: HashMap<u32, HeroInfo>,
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

    /// A hero by id or name: an exact name, else the only name starting with or containing the
    /// query (case-insensitive). Otherwise an `invalid_argument` naming up to 5 close heroes.
    pub fn find_hero(&self, query: &str) -> Result<u32> {
        let q = query.trim().to_lowercase();
        if let Ok(id) = q.parse::<u32>() {
            return if self.heroes.contains_key(&id) {
                Ok(id)
            } else {
                Err(Problem::invalid_argument(format!(
                    "no hero has id {id}; the deadlock://heroes resource lists them"
                )))
            };
        }
        let named = |pred: &dyn Fn(&str) -> bool| -> Vec<u32> {
            let mut ids: Vec<u32> = self
                .heroes
                .iter()
                .filter(|(_, n)| pred(&n.to_lowercase()))
                .map(|(&id, _)| id)
                .collect();
            ids.sort_unstable();
            ids
        };
        let mut found = named(&|n| n == q);
        if found.is_empty() {
            found = named(&|n| n.starts_with(&q));
        }
        if found.is_empty() {
            found = named(&|n| n.contains(&q));
        }
        if let [id] = found[..] {
            return Ok(id);
        }
        let close: Vec<String> = if found.is_empty() {
            let mut all: Vec<&String> = self.heroes.values().collect();
            all.sort_by_key(|n| (distance(&n.to_lowercase(), &q), n.as_str()));
            all.into_iter().take(5).cloned().collect()
        } else {
            found.iter().take(5).map(|&id| self.hero(id)).collect()
        };
        let what = if found.is_empty() {
            "no hero is named like"
        } else {
            "more than one hero matches"
        };
        Err(Problem::invalid_argument(format!(
            "{what} {query:?}; did you mean: {}?",
            close.join(", ")
        )))
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

/// Levenshtein distance, for suggesting close names.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let next = (prev + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
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
    let mut hero_info = HashMap::new();
    let heroes = heroes
        .into_iter()
        .filter_map(|h| {
            let id = u32::try_from(h.id).ok()?;
            let d = h.description;
            hero_info.insert(
                id,
                HeroInfo {
                    role: d.role.as_deref().map(strip_tags),
                    playstyle: d.playstyle.as_deref().map(strip_tags),
                    lore: d.lore.as_deref().map(strip_tags),
                    selectable: h.player_selectable,
                },
            );
            Some((id, h.name))
        })
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
        hero_info,
        items,
        ranks,
        lanes,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(names: &[(u32, &str)]) -> Catalog {
        Catalog {
            version: 1,
            heroes: names.iter().map(|&(id, n)| (id, n.to_owned())).collect(),
            hero_info: HashMap::new(),
            items: HashMap::new(),
            ranks: HashMap::new(),
            lanes: Vec::new(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn finds_heroes_by_id_and_name() {
        let cat = catalog(&[
            (1, "Infernus"),
            (12, "Kelvin"),
            (18, "Mo & Krill"),
            (20, "Ivy"),
            (21, "Kali"),
        ]);
        assert_eq!(cat.find_hero("12").unwrap(), 12);
        assert_eq!(cat.find_hero(" kelvin ").unwrap(), 12);
        assert_eq!(cat.find_hero("mo").unwrap(), 18);
        assert_eq!(cat.find_hero("krill").unwrap(), 18);
        assert!(cat.find_hero("99").is_err());
        let ambiguous = cat.find_hero("k").unwrap_err().message();
        assert!(
            ambiguous.contains("Kelvin") && ambiguous.contains("Kali"),
            "{ambiguous}"
        );
        let typo = cat.find_hero("kelvn").unwrap_err().message();
        assert!(typo.contains("did you mean: Kelvin"), "{typo}");
    }
}
