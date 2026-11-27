//! Resources (hero cards, the hero and item lists, match summaries) and hero id completion.

use rmcp::ErrorData;
use rmcp::model::{CompletionInfo, Resource, ResourceTemplate};
use serde::Serialize;

use crate::contracts::api::ItemSlotType;
use crate::domain::assets::{Catalog, catalog};
use crate::domain::items::ItemKind;
use crate::problem::{Kind, Problem};
use crate::tools::{game, matches};

#[derive(Serialize)]
pub(super) struct HeroResource {
    id: u32,
    name: String,
    abilities: Vec<String>,
}

#[derive(Serialize)]
struct ItemResource<'a> {
    id: u32,
    name: &'a str,
    kind: ItemKind,
    hero: Option<String>,
    slot: Option<ItemSlotType>,
    tier: Option<i32>,
    cost: Option<i64>,
}

const HEROES: &str = "deadlock://heroes";
const ITEMS: &str = "deadlock://items";
pub(super) const HERO_TEMPLATE: &str = "deadlock://hero/{id}";
const MATCH_TEMPLATE: &str = "deadlock://match/{match_id}";

/// The part of a template before its parameter, e.g. `deadlock://hero/`.
fn prefix(template: &str) -> &str {
    template.split_once('{').map_or(template, |(p, _)| p)
}

fn hero_uri(id: u32) -> String {
    format!("{}{id}", prefix(HERO_TEMPLATE))
}

/// The resources that are always there; `resources` adds one per hero.
fn static_resources() -> [Resource; 2] {
    [
        Resource::new(HEROES, "heroes")
            .with_description("Every playable hero with its id and abilities (JSON)")
            .with_mime_type("application/json"),
        Resource::new(ITEMS, "items")
            .with_description("Every shop item with id, slot, tier and cost (JSON)")
            .with_mime_type("application/json"),
    ]
}

pub(super) fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(HERO_TEMPLATE, "hero")
            .with_title("Hero card")
            .with_description(format!(
                "A hero's role, playstyle, lore and abilities with their tooltips (JSON). \
                 Hero ids are in {HEROES} or from game_lookup."
            ))
            .with_mime_type("application/json"),
        ResourceTemplate::new(MATCH_TEMPLATE, "match")
            .with_title("Match summary")
            .with_description("The match_summary answer for a match id (JSON)")
            .with_mime_type("application/json"),
    ]
}

/// What a resource URI names.
#[derive(Debug, PartialEq)]
enum Target {
    Heroes,
    Items,
    Hero(u32),
    Match(u64),
}

fn no_hero(uri: &str) -> ErrorData {
    ErrorData::resource_not_found(format!("no hero {uri}; {HEROES} lists the hero ids"), None)
}

fn target(uri: &str) -> Result<Target, ErrorData> {
    if let Some(id) = uri.strip_prefix(prefix(MATCH_TEMPLATE)) {
        return id.parse().map(Target::Match).map_err(|_| {
            ErrorData::invalid_params(format!("{uri}: a match id is a number"), None)
        });
    }
    if let Some(id) = uri.strip_prefix(prefix(HERO_TEMPLATE)) {
        return id.parse().map(Target::Hero).map_err(|_| no_hero(uri));
    }
    match uri {
        HEROES => Ok(Target::Heroes),
        ITEMS => Ok(Target::Items),
        _ => Err(ErrorData::resource_not_found(
            format!("no resource {uri}"),
            None,
        )),
    }
}

/// A problem as an MCP error: a missing thing is `resource_not_found`, a bad URI `invalid_params`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "taken by value to fit `map_err`"
)]
fn resource_error(p: Problem) -> ErrorData {
    match p.kind {
        Kind::NotFound => ErrorData::resource_not_found(p.message(), None),
        Kind::InvalidArgument => ErrorData::invalid_params(p.message(), None),
        _ => ErrorData::internal_error(p.message(), None),
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<String, ErrorData> {
    serde_json::to_string(value).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

/// The playable heroes by name.
fn playable_by_name(cat: &Catalog) -> Vec<(u32, &String)> {
    let mut heroes: Vec<(u32, &String)> = cat.playable().collect();
    heroes.sort_by(|a, b| a.1.cmp(b.1));
    heroes
}

pub(super) fn resource_text(uri: &str) -> Result<String, ErrorData> {
    match target(uri)? {
        Target::Match(id) => to_json(&matches::summary(id).map_err(resource_error)?),
        Target::Hero(id) => {
            let card = game::hero_card(id).map_err(resource_error)?;
            to_json(&card.ok_or_else(|| no_hero(uri))?)
        }
        Target::Heroes => {
            let cat = catalog().map_err(resource_error)?;
            let heroes: Vec<HeroResource> = playable_by_name(&cat)
                .into_iter()
                .map(|(id, name)| {
                    let abilities: Vec<String> = cat
                        .abilities(id)
                        .into_iter()
                        .map(|i| i.name.clone())
                        .collect();
                    HeroResource {
                        id,
                        name: name.clone(),
                        abilities,
                    }
                })
                .collect();
            to_json(&heroes)
        }
        Target::Items => {
            let cat = catalog().map_err(resource_error)?;
            let mut items: Vec<ItemResource<'_>> = cat
                .items
                .values()
                .filter(|i| i.kind == ItemKind::Upgrade)
                .map(|i| ItemResource {
                    id: i.id,
                    name: &i.name,
                    kind: i.kind,
                    hero: i.hero.map(|h| cat.hero(h)),
                    slot: i.slot,
                    tier: i.tier,
                    cost: i.cost,
                })
                .collect();
            items.sort_by(|a, b| (a.slot, a.tier, a.name).cmp(&(b.slot, b.tier, b.name)));
            to_json(&items)
        }
    }
}

/// Hero ids for a typed name or id: name prefix matches, then name substring matches, then id
/// prefix matches, each by name, at most 100.
pub(super) fn hero_completions(typed: &str) -> Result<Vec<String>, ErrorData> {
    let cat = catalog().map_err(resource_error)?;
    let q = typed.trim().to_lowercase();
    let mut found: Vec<(u8, &String, u32)> = cat
        .playable()
        .filter_map(|(id, name)| {
            let n = name.to_lowercase();
            let rank = if n.starts_with(&q) {
                0
            } else if n.contains(&q) {
                1
            } else if id.to_string().starts_with(&q) {
                2
            } else {
                return None;
            };
            Some((rank, name, id))
        })
        .collect();
    found.sort();
    Ok(found
        .into_iter()
        .take(CompletionInfo::MAX_VALUES)
        .map(|(_, _, id)| id.to_string())
        .collect())
}

/// The static resources and every playable hero as its own resource, so clients can show names.
pub(super) fn resources() -> Result<Vec<Resource>, ErrorData> {
    let cat = catalog().map_err(resource_error)?;
    let heroes = playable_by_name(&cat).into_iter().map(|(id, name)| {
        let r = Resource::new(hero_uri(id), name.to_lowercase())
            .with_title(name.clone())
            .with_mime_type("application/json");
        match cat.hero_info.get(&id).and_then(|h| h.role.as_deref()) {
            Some(role) => r.with_description(role),
            None => r,
        }
    });
    Ok(static_resources().into_iter().chain(heroes).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every URI the server lists or advertises routes back to what it names.
    #[test]
    fn listed_uris_route_back() {
        let routed: Vec<Option<Target>> = static_resources()
            .iter()
            .map(|r| target(&r.uri).ok())
            .collect();
        assert_eq!(routed, [Some(Target::Heroes), Some(Target::Items)]);
        assert_eq!(hero_uri(7), "deadlock://hero/7");
        assert_eq!(target(&hero_uri(7)).ok(), Some(Target::Hero(7)));
        let filled: Vec<Option<Target>> = templates()
            .iter()
            .map(|t| target(&format!("{}7", prefix(&t.uri_template))).ok())
            .collect();
        assert_eq!(filled, [Some(Target::Hero(7)), Some(Target::Match(7))]);
        assert!(target("deadlock://hero/kelvin").is_err());
        assert!(target("deadlock://match/x").is_err());
        assert!(target("deadlock://nothing").is_err());
    }
}
