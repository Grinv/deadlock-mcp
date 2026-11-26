//! `game_lookup` (heroes, abilities and items by name or id), the hero card resource and
//! `patches`.

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api::FeedItem;
use crate::domain::assets::catalog;
use crate::domain::format::{bounded, plain, strip_tags};
use crate::domain::items::{Item, ItemKind};
use crate::http::Source;
use crate::problem::{Problem, Result};

const LIMIT: usize = 15;

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Hero,
    Ability,
    Weapon,
    /// A shop item.
    Upgrade,
}

impl From<ItemKind> for Kind {
    fn from(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Ability => Self::Ability,
            ItemKind::Weapon => Self::Weapon,
            ItemKind::Upgrade => Self::Upgrade,
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub struct Entry {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    /// The hero an ability or weapon belongs to.
    pub hero: Option<String>,
    /// Shop category of an upgrade: `weapon`, `spirit` or `vitality`.
    pub slot: Option<String>,
    pub tier: Option<i32>,
    pub cost: Option<i64>,
    /// A hero's abilities.
    pub abilities: Vec<String>,
    /// The game's tooltip text (cut at 600 characters); a hero's role.
    pub description: Option<String>,
    /// What each of an ability's three upgrades (1, 2, 3 ability points) adds.
    pub upgrades: Vec<String>,
    /// The tooltip's labelled values at base level, e.g. "Cooldown: 28s" (at most 12).
    pub properties: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Lookup {
    pub results: Vec<Entry>,
    /// More matched than shown; use a longer name to narrow the search.
    pub truncated: bool,
    pub sources: Vec<Source>,
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
        kind: i.kind.into(),
        hero: i.hero.map(|h| cat.hero(h)),
        slot: i.slot.clone(),
        tier: i.tier,
        cost: i.cost,
        abilities: Vec::new(),
        description: i.description.clone(),
        upgrades: i.upgrades.clone(),
        properties: i.properties.clone(),
    };
    let hero_entry = |id: u32, name: &str| {
        let mut abilities: Vec<&Item> = cat
            .items
            .values()
            .filter(|i| i.hero == Some(id) && i.kind == ItemKind::Ability)
            .collect();
        abilities.sort_by_key(|i| i.name.as_str());
        Entry {
            id,
            name: name.to_owned(),
            kind: Kind::Hero,
            hero: None,
            slot: None,
            tier: None,
            cost: None,
            abilities: abilities
                .into_iter()
                .map(|i| format!("{} ({})", i.name, i.id))
                .collect(),
            description: cat.hero_info.get(&id).and_then(|h| h.role.clone()),
            upgrades: Vec::new(),
            properties: Vec::new(),
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
    Ok(Lookup {
        results,
        truncated,
        sources: cat.sources.clone(),
    })
}

#[derive(Serialize, JsonSchema)]
pub struct AbilityCard {
    pub id: u32,
    pub name: String,
    pub description: Option<String>,
    /// What each of the three upgrades adds.
    pub upgrades: Vec<String>,
    pub properties: Vec<String>,
}

/// The `deadlock://hero/{id}` resource.
#[derive(Serialize, JsonSchema)]
pub struct HeroCard {
    pub id: u32,
    pub name: String,
    pub role: Option<String>,
    pub playstyle: Option<String>,
    /// The game's short lore text; the wiki's lore page has the full story.
    pub lore: Option<String>,
    pub lore_page: String,
    pub abilities: Vec<AbilityCard>,
    pub sources: Vec<Source>,
}

/// `None` when the catalog has no hero `id`.
pub fn hero_card(id: u32) -> Result<Option<HeroCard>> {
    let cat = catalog()?;
    let Some(name) = cat.heroes.get(&id) else {
        return Ok(None);
    };
    let info = cat.hero_info.get(&id);
    // Abilities without tooltip text are helpers (melee, internal parts of a signature ability).
    let mut abilities: Vec<&Item> = cat
        .items
        .values()
        .filter(|i| i.hero == Some(id) && i.kind == ItemKind::Ability && i.description.is_some())
        .collect();
    abilities.sort_by_key(|i| i.name.as_str());
    Ok(Some(HeroCard {
        id,
        name: name.clone(),
        role: info.and_then(|h| h.role.clone()),
        playstyle: info.and_then(|h| h.playstyle.clone()),
        lore: info.and_then(|h| h.lore.clone()),
        lore_page: format!("{}/Lore", crate::sources::wiki::page_url(name)),
        abilities: abilities
            .into_iter()
            .map(|i| AbilityCard {
                id: i.id,
                name: i.name.clone(),
                description: i.description.clone(),
                upgrades: i.upgrades.clone(),
                properties: i.properties.clone(),
            })
            .collect(),
        sources: cat.sources.clone(),
    }))
}

/// Patch notes longer than this are cut.
const MAX_NOTES: usize = 12_000;

#[derive(Serialize, JsonSchema)]
pub struct Patch {
    pub title: String,
    pub date: String,
    /// `forum` (the official changelog) or `steam` (Steam news, also events and announcements).
    pub source: &'static str,
    pub link: String,
}

#[derive(Serialize, JsonSchema)]
pub struct PatchNotes {
    pub title: String,
    pub date: String,
    pub link: String,
    pub text: String,
    /// The notes were cut at 12,000 characters; the link has the rest.
    pub truncated: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct PatchFeed {
    /// Newest first; empty when `link` asked for one patch's notes.
    pub patches: Vec<Patch>,
    pub notes: Option<PatchNotes>,
    pub sources: Vec<Source>,
}

/// `(source, title, date, link, content)` of a feed entry.
fn fields(f: &FeedItem) -> (&'static str, &String, &String, &String, &String) {
    match f {
        FeedItem::ForumPatch {
            title,
            pub_date,
            link,
            content,
            ..
        } => ("forum", title, pub_date, link, content),
        FeedItem::SteamNews {
            title,
            pub_date,
            link,
            content,
            ..
        } => ("steam", title, pub_date, link, content),
    }
}

pub fn patches(link: Option<&str>, limit: usize) -> Result<PatchFeed> {
    let (feed, source) = crate::sources::api::patches()?;
    let day = |d: &str| d.get(..10).unwrap_or(d).to_owned();
    if let Some(wanted) = link {
        let Some((_, title, date, link, content)) =
            feed.iter().map(fields).find(|f| f.3 == wanted.trim())
        else {
            return Err(Problem::not_found(format!(
                "no patch with link {wanted:?} in the feed; call patches without a link to list them"
            )));
        };
        let (text, truncated) = bounded(&plain(&strip_tags(content)), MAX_NOTES);
        return Ok(PatchFeed {
            patches: Vec::new(),
            notes: Some(PatchNotes {
                title: plain(title.trim()),
                date: day(date),
                link: link.clone(),
                text,
                truncated,
            }),
            sources: vec![source],
        });
    }
    Ok(PatchFeed {
        patches: feed
            .iter()
            .map(fields)
            .take(limit)
            .map(|(source, title, date, link, _)| Patch {
                title: plain(title.trim()),
                date: day(date),
                source,
                link: link.clone(),
            })
            .collect(),
        notes: None,
        sources: vec![source],
    })
}
