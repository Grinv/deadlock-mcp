//! The game catalog from the Deadlock API's asset endpoints (built from the game files): heroes,
//! items (abilities, shop items, weapons) and rank names. Parsed once and kept in memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::problem::Result;
use crate::sources::api;

/// The on-disk copy is revalidated with its `ETag`; this only saves re-parsing 6.5 MB of JSON.
const MEMORY_TTL: Duration = Duration::from_secs(3600);

pub struct Item {
    pub id: u32,
    pub name: String,
    /// `ability`, `upgrade` (shop item) or `weapon`.
    pub kind: String,
    /// Shop category of an upgrade: `weapon`, `spirit` or `vitality`.
    pub slot: Option<String>,
    pub hero: Option<u32>,
    pub tier: Option<u64>,
    pub cost: Option<u64>,
}

pub struct Catalog {
    pub heroes: HashMap<u32, String>,
    pub items: HashMap<u32, Item>,
    ranks: HashMap<u64, String>,
}

impl Catalog {
    pub fn hero(&self, id: u64) -> String {
        u32::try_from(id)
            .ok()
            .and_then(|id| self.heroes.get(&id).cloned())
            .unwrap_or_else(|| format!("hero {id}"))
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        u32::try_from(id).ok().and_then(|id| self.items.get(&id))
    }

    /// `tier * 10 + subrank`, e.g. 116 = "Eternus VI"; 0 = unranked (Obscurus).
    pub fn rank(&self, badge: u64) -> String {
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
}

static CACHE: Mutex<Option<(Instant, Arc<Catalog>)>> = Mutex::new(None);

pub fn catalog() -> Result<Arc<Catalog>> {
    if let Ok(guard) = CACHE.lock()
        && let Some((at, catalog)) = guard.as_ref()
        && at.elapsed() < MEMORY_TTL
    {
        return Ok(Arc::clone(catalog));
    }
    let id32 = |v: &serde_json::Value| v.as_u64().and_then(|i| u32::try_from(i).ok());
    let mut catalog = Catalog {
        heroes: HashMap::new(),
        items: HashMap::new(),
        ranks: HashMap::new(),
    };
    for h in api::heroes()?.0.as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (id32(&h["id"]), h["name"].as_str()) {
            catalog.heroes.insert(id, name.to_owned());
        }
    }
    for i in api::items()?.0.as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (id32(&i["id"]), i["name"].as_str()) {
            let item = Item {
                id,
                name: name.to_owned(),
                kind: i["type"].as_str().unwrap_or_default().to_owned(),
                slot: i["item_slot_type"].as_str().map(str::to_owned),
                hero: id32(&i["hero"]),
                tier: i["item_tier"].as_u64(),
                cost: i["cost"].as_u64(),
            };
            catalog.items.insert(id, item);
        }
    }
    for r in api::ranks()?.0.as_array().into_iter().flatten() {
        if let (Some(tier), Some(name)) = (r["tier"].as_u64(), r["name"].as_str()) {
            catalog.ranks.insert(tier, name.to_owned());
        }
    }
    let catalog = Arc::new(catalog);
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((Instant::now(), Arc::clone(&catalog)));
    }
    Ok(catalog)
}
