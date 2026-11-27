//! The item model (abilities, weapons, shop items) and its tooltip text and property formatting.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::api;
use crate::domain::format::{clip, strip_tags};

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
    /// The game's tooltip text, as plain text.
    pub description: Option<String>,
    /// What each of an ability's three upgrades adds.
    pub upgrades: Vec<String>,
    /// Labelled values shown in the tooltip, e.g. "Cooldown: 28s".
    pub properties: Vec<String>,
}

/// Longer tooltips are cut here.
pub(crate) const MAX_TEXT: usize = 600;
/// At most this many properties per item.
const MAX_PROPERTIES: usize = 12;

/// Tooltip markup as plain text, cut at `MAX_TEXT` characters.
fn text(parts: &[&Option<String>]) -> Option<String> {
    let joined: Vec<String> = parts
        .iter()
        .filter_map(|p| p.as_deref())
        .map(strip_tags)
        .filter(|t| !t.is_empty())
        .collect();
    let t = clip(&joined.join("\n"), MAX_TEXT);
    (!t.is_empty()).then_some(t)
}

/// "Cooldown: 28s" for a property the tooltip shows. `0`, `-1` and the property's `disable_value`
/// mean the property is unused; a value with no number is left out.
fn property(
    label: Option<&str>,
    value: Option<&api::StringOrNumber>,
    prefix: Option<&str>,
    postfix: Option<&str>,
    disabled: Option<&str>,
) -> Option<String> {
    let label = label.filter(|l| !l.is_empty())?;
    let value = value?.to_string();
    let number: f64 = value
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()?;
    #[expect(clippy::float_cmp, reason = "the game writes these markers exactly")]
    if number == 0.0 || number == -1.0 || Some(value.as_str()) == disabled {
        return None;
    }
    let prefix = match prefix {
        Some("{s:sign}") if number > 0.0 => "+",
        Some(p) if !p.contains('{') => p,
        _ => "",
    };
    let postfix = postfix.filter(|p| !p.contains('{') && !value.ends_with(*p));
    Some(format!("{label}: {prefix}{value}{}", postfix.unwrap_or("")))
}

/// Tooltip properties in a stable order (by the game's property name), at most `MAX_PROPERTIES`.
fn properties<P>(
    props: Option<HashMap<String, P>>,
    show: impl Fn(&P) -> Option<String>,
) -> Vec<String> {
    let mut props: Vec<(String, P)> = props.unwrap_or_default().into_iter().collect();
    props.sort_by(|a, b| a.0.cmp(&b.0));
    props
        .iter()
        .filter_map(|(_, p)| show(p))
        .take(MAX_PROPERTIES)
        .collect()
}

impl From<api::Item> for Item {
    fn from(item: api::Item) -> Self {
        let id32 = |id: i64| u32::try_from(id).unwrap_or_default();
        match item {
            api::Item::Ability {
                id,
                name,
                hero,
                description: d,
                properties: props,
                ..
            } => Self {
                id: id32(id),
                name,
                kind: ItemKind::Ability,
                slot: None,
                hero: hero.map(id32),
                tier: None,
                cost: None,
                description: text(&[&d.desc, &d.active, &d.passive]),
                upgrades: [&d.t1_desc, &d.t2_desc, &d.t3_desc]
                    .into_iter()
                    .filter_map(|t| text(&[t]))
                    .collect(),
                properties: properties(props, |p: &api::ItemProperty| {
                    property(
                        p.label.as_deref(),
                        p.value.as_ref(),
                        p.prefix.as_deref(),
                        p.postfix.as_deref(),
                        p.disable_value.as_deref(),
                    )
                }),
            },
            api::Item::Weapon { id, name, hero, .. } => Self {
                id: id32(id),
                name,
                kind: ItemKind::Weapon,
                slot: None,
                hero: hero.map(id32),
                tier: None,
                cost: None,
                description: None,
                upgrades: Vec::new(),
                properties: Vec::new(),
            },
            api::Item::Upgrade {
                id,
                name,
                hero,
                item_slot_type,
                item_tier,
                cost,
                description: d,
                properties: props,
                tooltip_sections,
                ..
            } => Self {
                id: id32(id),
                name,
                kind: ItemKind::Upgrade,
                slot: Some(item_slot_type.to_string()),
                hero: hero.map(id32),
                tier: Some(item_tier),
                cost,
                // Some items (e.g. Cheat Death) carry their text only in the tooltip sections.
                description: d
                    .and_then(|d| text(&[&d.desc, &d.desc2, &d.active, &d.passive]))
                    .or_else(|| {
                        let parts: Vec<Option<String>> = tooltip_sections
                            .iter()
                            .flatten()
                            .flat_map(|s| s.section_attributes.iter().flatten())
                            .map(|a| a.loc_string.clone())
                            .collect();
                        text(&parts.iter().collect::<Vec<_>>())
                    }),
                upgrades: Vec::new(),
                properties: properties(props, |p: &api::UpgradeProperty| {
                    property(
                        p.label.as_deref(),
                        p.value.as_ref(),
                        p.prefix.as_deref(),
                        p.postfix.as_deref(),
                        p.disable_value.as_deref(),
                    )
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Item {
        /// An item for tests of the tools; the optional fields are empty.
        pub fn test(id: u32, name: &str, kind: ItemKind) -> Self {
            Self {
                id,
                name: name.to_owned(),
                kind,
                slot: None,
                hero: None,
                tier: None,
                cost: None,
                description: None,
                upgrades: Vec::new(),
                properties: Vec::new(),
            }
        }
    }

    #[test]
    fn shows_only_used_properties() {
        let n = |v: f64| Some(api::StringOrNumber::Double(v));
        let s = |v: &str| Some(api::StringOrNumber::String(v.to_owned()));
        assert_eq!(
            property(
                Some("Cooldown"),
                n(28.0).as_ref(),
                None,
                Some("s"),
                Some("0")
            )
            .as_deref(),
            Some("Cooldown: 28s")
        );
        assert_eq!(
            property(
                Some("Beam Length"),
                s("25m").as_ref(),
                None,
                Some("m"),
                None
            )
            .as_deref(),
            Some("Beam Length: 25m")
        );
        assert_eq!(
            property(
                Some("Max Ammo"),
                s("30").as_ref(),
                Some("{s:sign}"),
                Some("%"),
                None
            )
            .as_deref(),
            Some("Max Ammo: +30%")
        );
        assert_eq!(
            property(
                Some("Charge Delay"),
                s("-1.0").as_ref(),
                None,
                Some("s"),
                Some("-2")
            ),
            None
        );
        assert_eq!(
            property(Some("Duration"), s("0").as_ref(), None, None, None),
            None
        );
        assert_eq!(property(None, s("5").as_ref(), None, None, None), None);
    }
}
