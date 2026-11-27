//! `game_lookup` (heroes, abilities and items by name or id), the hero card resource and
//! `patches`.

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api::FeedItem;
use crate::domain::assets::{Catalog, catalog};
use crate::domain::format::{bounded, clip, plain, strip_tags};
use crate::domain::items::{self, Item, ItemKind};
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
    /// The game's tooltip text of an ability or item (cut at 600 characters).
    pub description: Option<String>,
    /// A hero's role in a line, e.g. "Sneaks in and sprays bullets". The game files leave it out
    /// for many heroes; `lore` is there for them.
    pub role: Option<String>,
    /// The start of a hero's lore (cut at 600 characters); `deadlock://hero/{id}` has all of it.
    pub lore: Option<String>,
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
    lookup_from(&cat, query)
}

fn lookup_from(cat: &Catalog, query: &str) -> Result<Lookup> {
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
        role: None,
        lore: None,
        upgrades: i.upgrades.clone(),
        properties: i.properties.clone(),
    };
    let hero_entry = |id: u32, name: &str| {
        let info = cat.hero_info.get(&id);
        let abilities = cat.abilities(id);
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
            description: None,
            role: info.and_then(|h| h.role.clone()),
            // Already plain text: cleaning it again would turn its line breaks into spaces.
            lore: info
                .and_then(|h| h.lore.as_deref())
                .map(|l| clip(l, items::MAX_TEXT)),
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
            .playable()
            .filter(|(_, n)| n.to_lowercase().contains(&q))
            .map(|(id, n)| hero_entry(id, n))
            .collect();
        let mut items: Vec<Entry> = cat
            .items
            .values()
            .filter(|i| i.name.to_lowercase().contains(&q))
            .map(item_entry)
            .collect();
        // Exact names first, then shorter (closer) names; ties by name and id, as the catalog's
        // maps iterate in a different order in every process and the list is cut at `LIMIT`.
        let rank = |e: &Entry| {
            (
                e.name.to_lowercase() != q,
                e.name.len(),
                e.name.clone(),
                e.id,
            )
        };
        heroes.sort_by_cached_key(rank);
        items.sort_by_cached_key(rank);
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
    let abilities = cat.abilities(id);
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
    patches_from(link, limit, &feed, vec![source])
}

fn patches_from(
    link: Option<&str>,
    limit: usize,
    feed: &[FeedItem],
    sources: Vec<Source>,
) -> Result<PatchFeed> {
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
            sources,
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
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_ranks_exact_then_short_names() {
        let mut cat = Catalog::test(&[(1, "Infernus"), (2, "Beamer"), (13, "Beam Ghost")]);
        for (id, name) in [
            (101, "Beam Blast"),
            (130, "Echo Beam"),
            (105, "Iron Beam"),
            (112, "Beam"),
            (120, "Echo Beam"),
            (140, "Shield"),
        ] {
            cat.items
                .insert(id, Item::test(id, name, ItemKind::Upgrade));
        }
        let found = |q: &str| {
            let l = lookup_from(&cat, q).unwrap();
            let names: Vec<_> = l.results.iter().map(|e| (e.id, e.name.clone())).collect();
            (names, l.truncated)
        };
        // Playable heroes first (13 is not); then the exact name, then shorter names, then by name
        // and id.
        assert_eq!(
            found(" BEAM "),
            (
                vec![
                    (2, "Beamer".to_owned()),
                    (112, "Beam".to_owned()),
                    (120, "Echo Beam".to_owned()),
                    (130, "Echo Beam".to_owned()),
                    (105, "Iron Beam".to_owned()),
                    (101, "Beam Blast".to_owned()),
                ],
                false
            )
        );
        // An id names a hero or an item, playable or not.
        assert_eq!(found("13"), (vec![(13, "Beam Ghost".to_owned())], false));
        assert_eq!(
            lookup_from(&cat, "zzz").err().unwrap().message(),
            "Not found: nothing named like \"zzz\"; try part of the name, e.g. \"beam\""
        );
        assert_eq!(
            lookup_from(&cat, "  ").err().unwrap().message(),
            "Invalid argument: query is empty; give a hero, ability or item name, or an id"
        );
    }

    #[test]
    fn lookup_cuts_at_the_limit() {
        let mut cat = Catalog::test(&[]);
        for id in 0..16 {
            cat.items.insert(
                id,
                Item::test(id, &format!("Beam {id:02}"), ItemKind::Upgrade),
            );
        }
        let l = lookup_from(&cat, "beam").unwrap();
        assert_eq!((l.results.len(), l.truncated), (15, true));
        assert_eq!(l.results[14].name, "Beam 14");
    }

    fn feed() -> Vec<FeedItem> {
        serde_json::from_str(include_str!("../../contracts/fixtures/patches.json")).unwrap()
    }

    #[test]
    fn patches_list_and_find_by_link() {
        let list = patches_from(None, 1, &feed(), Vec::new()).unwrap();
        let rows: Vec<_> = list
            .patches
            .iter()
            .map(|p| (p.title.as_str(), p.date.as_str(), p.source, p.link.as_str()))
            .collect();
        assert_eq!(
            rows,
            [(
                "09-29-2026",
                "2026-10-15",
                "forum",
                "https://forums.playdeadlock.com/threads/09-29-2026.166726/"
            )]
        );
        assert!(list.notes.is_none());

        let link = "https://store.steampowered.com/news/app/1422450/view/680766201707627248";
        let one = patches_from(Some(&format!(" {link} ")), 10, &feed(), Vec::new()).unwrap();
        let notes = one.notes.unwrap();
        assert!(one.patches.is_empty());
        assert_eq!(
            (
                notes.title.as_str(),
                notes.date.as_str(),
                notes.link.as_str()
            ),
            ("Ranked - Beta Season 2", "2026-11-05", link)
        );
        assert!(!notes.truncated);

        assert_eq!(
            patches_from(Some("https://example.com/"), 10, &feed(), Vec::new())
                .err()
                .unwrap()
                .message(),
            "Not found: no patch with link \"https://example.com/\" in the feed; call patches without a link to list them"
        );
    }
}
