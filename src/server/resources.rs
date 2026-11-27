//! Resources (hero cards, the hero and item lists, match summaries) and hero id completion.

use rmcp::ErrorData;
use rmcp::model::{CompletionInfo, Resource};
use serde::Serialize;

use crate::domain::assets::catalog;
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
    slot: Option<&'a str>,
    tier: Option<i32>,
    cost: Option<i64>,
}

pub(super) const HERO_TEMPLATE: &str = "deadlock://hero/{id}";
pub(super) const MATCH_TEMPLATE: &str = "deadlock://match/{match_id}";

/// A problem as an MCP error: a missing thing is `resource_not_found`, a bad URI `invalid_params`.
fn resource_error(p: &Problem) -> ErrorData {
    match p.kind {
        Kind::NotFound => ErrorData::resource_not_found(p.message(), None),
        Kind::InvalidArgument => ErrorData::invalid_params(p.message(), None),
        _ => ErrorData::internal_error(p.message(), None),
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<String, ErrorData> {
    serde_json::to_string(value).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

pub(super) fn resource_text(uri: &str) -> Result<String, ErrorData> {
    if let Some(id) = uri.strip_prefix("deadlock://match/") {
        let id: u64 = id.parse().map_err(|_| {
            ErrorData::invalid_params(format!("{uri}: a match id is a number"), None)
        })?;
        return to_json(&matches::summary(id).map_err(|p| resource_error(&p))?);
    }
    if let Some(id) = uri.strip_prefix("deadlock://hero/") {
        let not_found = || {
            ErrorData::resource_not_found(
                format!("no hero {uri}; deadlock://heroes lists the hero ids"),
                None,
            )
        };
        let id: u32 = id.parse().map_err(|_| not_found())?;
        let card = game::hero_card(id).map_err(|p| resource_error(&p))?;
        return to_json(&card.ok_or_else(not_found)?);
    }
    let cat = catalog().map_err(|p| resource_error(&p))?;
    let json = match uri {
        "deadlock://heroes" => {
            let mut heroes: Vec<HeroResource> = cat
                .playable()
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
            heroes.sort_by(|a, b| a.name.cmp(&b.name));
            serde_json::to_string(&heroes)
        }
        "deadlock://items" => {
            let mut items: Vec<ItemResource<'_>> = cat
                .items
                .values()
                .filter(|i| i.kind == ItemKind::Upgrade)
                .map(|i| ItemResource {
                    id: i.id,
                    name: &i.name,
                    kind: i.kind,
                    hero: i.hero.map(|h| cat.hero(h)),
                    slot: i.slot.as_deref(),
                    tier: i.tier,
                    cost: i.cost,
                })
                .collect();
            items.sort_by(|a, b| (a.slot, a.tier, a.name).cmp(&(b.slot, b.tier, b.name)));
            serde_json::to_string(&items)
        }
        _ => {
            return Err(ErrorData::resource_not_found(
                format!("no resource {uri}"),
                None,
            ));
        }
    };
    json.map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

/// Hero ids for a typed name or id: name prefix matches, then name substring matches, then id
/// prefix matches, each by name, at most 100.
pub(super) fn hero_completions(typed: &str) -> Result<Vec<String>, ErrorData> {
    let cat = catalog().map_err(|p| resource_error(&p))?;
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

/// Every playable hero as its own resource, so clients can show names.
pub(super) fn hero_resources() -> Result<Vec<Resource>, ErrorData> {
    let cat = catalog().map_err(|p| resource_error(&p))?;
    let mut heroes: Vec<(u32, &String)> = cat.playable().collect();
    heroes.sort_by(|a, b| a.1.cmp(b.1));
    Ok(heroes
        .into_iter()
        .map(|(id, name)| {
            let r = Resource::new(format!("deadlock://hero/{id}"), name.to_lowercase())
                .with_title(name.clone())
                .with_mime_type("application/json");
            match cat.hero_info.get(&id).and_then(|h| h.role.as_deref()) {
                Some(role) => r.with_description(role),
                None => r,
            }
        })
        .collect())
}
