//! The categories panel (`docs/07-designer.md` §7): the chart's categories
//! with what uses each, and the colour picker's contrast warning, which is
//! the compiler's own W107 rule (03 §3.2).

use std::collections::BTreeMap;

use cavea_compiler::document::{BookAs, Category, Node};
use cavea_core::limits::MAX_LABEL_LEN;
use serde::Serialize;

use crate::command::{CommandError, category};
use crate::document::Document;
use crate::index::SpatialIndex;

/// A colour against the built-in themes' backgrounds (W107).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contrast {
    /// WCAG 2 contrast ratios against the light and dark canvas.
    pub light: f64,
    pub dark: f64,
    /// Below 3:1 on either (11 §6 non-text minimum): weak on that canvas.
    pub weak: bool,
}

/// `#RRGGBB`, as the compiler reads it (anything else is E005).
pub(crate) fn rgb(colour: &str) -> Option<[u8; 3]> {
    let hex = colour.strip_prefix('#').filter(|h| h.len() == 6)?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    let [_, r, g, b] = v.to_be_bytes();
    Some([r, g, b])
}

/// How `colour` stands against the light and dark canvas; `None` if it is
/// not `#RRGGBB`.
pub fn contrast(colour: &str) -> Option<Contrast> {
    let l = cavea_compiler::luminance(rgb(colour)?);
    let ratio = |name: &str| {
        let bg = cavea_compiler::BACKGROUNDS.iter().find(|b| b.0 == name);
        bg.map_or(f64::INFINITY, |b| {
            cavea_compiler::contrast(l, cavea_compiler::luminance(b.1))
        })
    };
    let (light, dark) = (ratio("light"), ratio("dark"));
    let weak = light.min(dark) < cavea_compiler::MIN_CONTRAST;
    Some(Contrast { light, dark, weak })
}

/// Whether two colours are too similar to tell apart (W107's "redmean"
/// distance below 40); `None` unless both are `#RRGGBB`.
pub fn too_similar(a: &str, b: &str) -> Option<bool> {
    Some(cavea_compiler::too_similar(rgb(a)?, rgb(b)?))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoriesView {
    pub categories: Vec<CategoryView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryView {
    pub key: String,
    pub label: String,
    pub color: String,
    pub accessible: bool,
    /// The session's active category (the brush's, and new seats').
    pub active: bool,
    /// Bookable objects whose category it is, hidden nodes included: each
    /// present seat (by its override, else its node, else its section), a
    /// table booked whole, a booth, an area.
    pub objects: u32,
    /// Node fields and seat overrides naming it; a category is in use while
    /// this is above 0.
    pub references: u32,
    /// The other categories whose colour is too similar to this one's.
    pub similar: Vec<String>,
    /// `None` when the colour is not `#RRGGBB`.
    pub contrast: Option<Contrast>,
}

/// References to `key`: node `category` fields and seat override categories.
pub(crate) fn references(doc: &Document, key: &str) -> u32 {
    let mut n = 0u32;
    for id in doc.ids() {
        let Some(node) = doc.node(id) else {
            continue;
        };
        if category(node).is_some_and(|c| c.as_deref() == Some(key)) {
            n = n.saturating_add(1);
        }
        let overrides = match node {
            Node::RowBlock(b) => &b.seat_overrides,
            Node::Row(r) => &r.seat_overrides,
            _ => continue,
        };
        let named = overrides
            .values()
            .filter(|o| o.category.as_deref() == Some(key));
        n = n.saturating_add(named.count() as u32);
    }
    n
}

fn add<'a>(map: &mut BTreeMap<&'a str, u32>, key: Option<&'a str>, n: u32) {
    if let Some(k) = key {
        let slot = map.entry(k).or_default();
        *slot = slot.saturating_add(n);
    }
}

pub(crate) fn view(doc: &Document, index: &SpatialIndex, active: Option<&str>) -> CategoriesView {
    let mut objects: BTreeMap<&str, u32> = BTreeMap::new();
    let mut refs: BTreeMap<&str, u32> = BTreeMap::new();
    for id in doc.ids() {
        let Some(node) = doc.node(id) else {
            continue;
        };
        let own = category(node).and_then(|c| c.as_deref());
        add(&mut refs, own, 1);
        let section = doc
            .parent(id)
            .and_then(|s| doc.node(s))
            .and_then(category)
            .and_then(|c| c.as_deref());
        let inherited = own.or(section);
        let overrides = match node {
            Node::RowBlock(b) => &b.seat_overrides,
            Node::Row(r) => &r.seat_overrides,
            Node::Table(t) => {
                let n = if t.book_as == BookAs::Table {
                    1
                } else {
                    t.seats
                };
                add(&mut objects, inherited, n);
                continue;
            }
            Node::Booth(_) | Node::Area(_) => {
                add(&mut objects, inherited, 1);
                continue;
            }
            _ => continue,
        };
        for o in overrides.values() {
            add(&mut refs, o.category.as_deref(), 1);
        }
        // Present seats, from the index's cached expansion: an override's
        // category counts only on a present seat of this node.
        let seats = index.seats(id).iter().filter(|s| !s.removed);
        if overrides.values().all(|o| o.category.is_none()) {
            add(&mut objects, inherited, seats.count() as u32);
        } else {
            for s in seats {
                let over = overrides.get(&s.oid).and_then(|o| o.category.as_deref());
                add(&mut objects, over.or(inherited), 1);
            }
        }
    }
    let cats = doc.categories();
    let categories = cats
        .iter()
        .map(|c| CategoryView {
            key: c.key.clone(),
            label: c.label.clone(),
            color: c.color.clone(),
            accessible: c.accessible,
            active: active == Some(c.key.as_str()),
            objects: objects.get(c.key.as_str()).copied().unwrap_or(0),
            references: refs.get(c.key.as_str()).copied().unwrap_or(0),
            similar: cats
                .iter()
                .filter(|o| o.key != c.key && too_similar(&o.color, &c.color) == Some(true))
                .map(|o| o.key.clone())
                .collect(),
            contrast: contrast(&c.color),
        })
        .collect();
    CategoriesView { categories }
}

/// A category from the panel: a key of 1 to 64 bytes, a label of at most
/// 64, and a `#RRGGBB` colour; anything else is refused.
pub(crate) fn check(c: &Category) -> Result<(), CommandError> {
    if c.key.is_empty() || c.key.len() > MAX_LABEL_LEN || c.label.len() > MAX_LABEL_LEN {
        return Err(CommandError::Limit("label bytes"));
    }
    if rgb(&c.color).is_none() {
        return Err(CommandError::InvalidValue("color"));
    }
    Ok(())
}
