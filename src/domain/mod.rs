//! Game knowledge independent of where the data comes from.

pub mod assets;
pub mod format;
pub mod items;
pub mod map;
pub mod outcome;
pub mod terms;

/// An API id as the catalog's `u32`; ids out of range become 0, which names no hero or item.
pub fn id32(id: i64) -> u32 {
    u32::try_from(id).unwrap_or_default()
}
