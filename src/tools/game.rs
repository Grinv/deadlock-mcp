//! `game_lookup`: heroes, abilities and items by name or id.

use schemars::JsonSchema;
use serde::Serialize;

use crate::domain::assets::{Item, catalog};
use crate::problem::{Problem, Result};

const LIMIT: usize = 15;

#[derive(Serialize, JsonSchema)]
pub struct Entry {
    pub id: u32,
    pub name: String,
    /// `hero`, `ability`, `upgrade` (shop item) or `weapon`.
    pub kind: String,
    /// The hero an ability or weapon belongs to.
    pub hero: Option<String>,
    /// Shop category of an upgrade: `weapon`, `spirit` or `vitality`.
    pub slot: Option<String>,
    pub tier: Option<u64>,
    pub cost: Option<u64>,
    /// A hero's abilities.
    pub abilities: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Lookup {
    pub results: Vec<Entry>,
    /// More matched than shown; use a longer name to narrow the search.
    pub truncated: bool,
}

pub fn lookup(query: &str) -> Result<Lookup> {
    let cat = catalog()?;
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Err(Problem::invalid_argument(
            "query is empty; give a hero, ability or item name, or an id",
        ));
    }
    let item_entry = |i: &Item| Entry {
        id: i.id,
        name: i.name.clone(),
        kind: i.kind.clone(),
        hero: i.hero.map(|h| cat.hero(u64::from(h))),
        slot: i.slot.clone(),
        tier: i.tier,
        cost: i.cost,
        abilities: Vec::new(),
    };
    let hero_entry = |id: u32, name: &str| {
        let mut abilities: Vec<&Item> = cat
            .items
            .values()
            .filter(|i| i.hero == Some(id) && i.kind == "ability")
            .collect();
        abilities.sort_by_key(|i| i.name.as_str());
        Entry {
            id,
            name: name.to_owned(),
            kind: "hero".to_owned(),
            hero: None,
            slot: None,
            tier: None,
            cost: None,
            abilities: abilities
                .into_iter()
                .map(|i| format!("{} ({})", i.name, i.id))
                .collect(),
        }
    };

    let mut results: Vec<Entry> = if let Ok(id) = q.parse::<u32>() {
        cat.heroes
            .get(&id)
            .map(|name| hero_entry(id, name))
            .into_iter()
            .chain(cat.items.get(&id).map(item_entry))
            .collect()
    } else {
        let mut heroes: Vec<Entry> = cat
            .heroes
            .iter()
            .filter(|(_, n)| n.to_lowercase().contains(&q))
            .map(|(&id, n)| hero_entry(id, n))
            .collect();
        let mut items: Vec<Entry> = cat
            .items
            .values()
            .filter(|i| i.name.to_lowercase().contains(&q))
            .map(item_entry)
            .collect();
        // Exact names first, then shorter (closer) names.
        let rank = |e: &Entry| (e.name.to_lowercase() != q, e.name.len());
        heroes.sort_by_key(rank);
        items.sort_by_key(rank);
        heroes.into_iter().chain(items).collect()
    };
    if results.is_empty() {
        return Err(Problem::not_found(format!(
            "nothing named like {query:?}; try part of the name, e.g. \"beam\""
        )));
    }
    let truncated = results.len() > LIMIT;
    results.truncate(LIMIT);
    Ok(Lookup { results, truncated })
}
