//! The JSON and the names that cross between the WASM and `@cavea/renderer`
//! (05 §3, §5): the engine's events as `View.takeEvents` writes them, the
//! object `View.object` writes, and the option values the SDK passes in.
//! `cargo xtask types` reads these definitions and writes
//! `packages/renderer/src/generated/engine.ts` (D-41, D-71), so the SDK's
//! types have one source.
//!
//! Compiled for tests only: the WASM writes its JSON by hand (`events.rs`)
//! and carries no serde. The tests below tie the three together. Each
//! conversion from the engine's protocol types is an exhaustive match or
//! destructuring, so a field or variant added there does not compile here
//! until it is added to these types (and the generated TypeScript then goes
//! stale: `cargo xtask types --check`); and the JSON `events.rs` writes for
//! every event must read back into these types, unknown fields refused, and
//! equal them.

use cavea_render::protocol::{self as p, BuiltinTheme, Event};
use serde::{Deserialize, Serialize};

/// An engine event, as `View.takeEvents` reports it (04 §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum EngineEvent {
    /// A chart was loaded.
    Ready(ChartInfo),
    /// Hold these labels, then answer with `holdOk` or `holdFailed`.
    HoldRequest { request: u32, labels: Vec<String> },
    /// Release these labels; there is no answer.
    ReleaseRequest { request: u32, labels: Vec<String> },
    /// `expiresAt`: when the buyer's holds expire (Unix ms), if known.
    SelectionChanged {
        expires_at: Option<u64>,
        selected: Vec<ObjectInfo>,
    },
    /// Rule `min`: the selection is not ready for checkout (`objects` is the
    /// selection). Any other rule (`max`, `not_free`, a server code):
    /// `objects` were refused and the selection is as it was.
    SelectionInvalid { rule: String, objects: Vec<String> },
    /// The selection meets the minimum again.
    SelectionValid,
    /// The object under the pointer and its centre in CSS pixels on the
    /// canvas; both `null` when it leaves every object.
    ObjectHover {
        object: Option<ObjectInfo>,
        at: Option<[f32; 2]>,
    },
    /// The keyboard focus shown and where the camera brings it to rest;
    /// both `null` when it is hidden.
    ObjectFocus {
        object: Option<ObjectInfo>,
        at: Option<[f32; 2]>,
    },
    /// A selected object was taken; it is no longer selected.
    ObjectUnavailable { label: String },
    /// The buyer's hold on these selected objects is gone (expired, or
    /// released elsewhere).
    HoldLost { labels: Vec<String> },
    /// A delta did not follow the state at `seq`: fetch a new snapshot.
    Resync { seq: Option<u64> },
    /// The camera moved, the level changed, or the zoom range did.
    CameraChanged {
        centre: [f32; 2],
        zoom: f32,
        /// The lowest and highest zoom, for the current chart and size
        /// (04 §13).
        range: [f32; 2],
        lod: Lod,
    },
    /// Draw this string and answer with `setText`; the host does, between
    /// frames (04 §12).
    TextRequest {
        id: u32,
        text: String,
        /// Device pixels per em.
        px: u16,
        weight: u16,
        /// A CSS font family.
        font: String,
    },
    /// Load this background image and answer with `setImage`, scaled down
    /// to at most `maxSide` pixels a side and `maxPixels` in all (04 §7).
    ImageRequest {
        id: u32,
        /// The asset's content hash: 32 lowercase hex digits.
        asset: String,
        max_side: u32,
        max_pixels: u32,
    },
}

/// One object as the engine reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectInfo {
    /// Full label, for example `"Orchestra-C-12"`.
    pub label: String,
    /// Category key.
    pub category: String,
    /// Section label; empty outside a section.
    pub section: String,
    pub row: String,
    pub seat: String,
    /// `SFLG` bits: 0 accessible, 1 companion, 2 restricted view, 3 aisle.
    pub flags: u8,
}

/// An object with its state, as `View.object` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectDetails {
    pub label: String,
    pub category: String,
    pub section: String,
    pub row: String,
    pub seat: String,
    pub flags: u8,
    /// The public status code: 0 available, 1 unavailable.
    pub status: u8,
    /// Its place in its row: the n-th of so many.
    pub place: [u32; 2],
}

/// Chart metadata, from the engine's `Ready` (04 §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartInfo {
    pub name: String,
    pub floors: Vec<FloorInfo>,
    pub sections: Vec<SectionInfo>,
    pub categories: Vec<CategoryInfo>,
    /// Bookable objects: seats, then tables booked whole, booths and areas.
    pub objects: u32,
    pub seats: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorInfo {
    pub name: String,
    pub level: i32,
    pub seats: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionInfo {
    pub label: String,
    /// Index into `floors`.
    pub floor: u32,
    pub seats: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryInfo {
    pub key: String,
    pub label: String,
    /// RGBA bytes.
    pub color: [u8; 4],
    /// Bookable objects in the category.
    pub objects: u32,
}

/// The level of detail (04 §10).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lod {
    Overview,
    CrossFade,
    Seats,
    Detail,
}

/// A built-in theme (04 §19), as `View.setTheme` takes its name.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    Light,
    Dark,
    HighContrast,
}

/// Which gestures move the map (04 §14).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GestureHandling {
    Greedy,
    Cooperative,
    None,
}

/// The event's JSON, or `None` for an event the host is not told about.
fn from_protocol(event: &Event) -> Option<EngineEvent> {
    let pointed = |o: &Option<(p::ObjectInfo, [f32; 2])>| match o {
        Some((object, at)) => (Some(object_info(object)), Some(*at)),
        None => (None, None),
    };
    Some(match event {
        Event::Ready(info) => EngineEvent::Ready(chart_info(info)),
        Event::HoldRequest { request, labels } => EngineEvent::HoldRequest {
            request: *request,
            labels: labels.clone(),
        },
        Event::ReleaseRequest { request, labels } => EngineEvent::ReleaseRequest {
            request: *request,
            labels: labels.clone(),
        },
        Event::SelectionChanged {
            selected,
            expires_at_ms,
        } => EngineEvent::SelectionChanged {
            expires_at: *expires_at_ms,
            selected: selected.iter().map(object_info).collect(),
        },
        Event::SelectionInvalid { rule, objects } => EngineEvent::SelectionInvalid {
            rule: rule.clone(),
            objects: objects.clone(),
        },
        Event::SelectionValid => EngineEvent::SelectionValid,
        Event::ObjectHover(o) => {
            let (object, at) = pointed(o);
            EngineEvent::ObjectHover { object, at }
        }
        Event::ObjectFocus(o) => {
            let (object, at) = pointed(o);
            EngineEvent::ObjectFocus { object, at }
        }
        Event::ObjectUnavailable { label } => EngineEvent::ObjectUnavailable {
            label: label.clone(),
        },
        Event::HoldLost { labels } => EngineEvent::HoldLost {
            labels: labels.clone(),
        },
        Event::Resync { seq } => EngineEvent::Resync { seq: *seq },
        Event::CameraChanged {
            centre,
            zoom,
            range,
            lod,
        } => EngineEvent::CameraChanged {
            centre: *centre,
            zoom: *zoom,
            range: *range,
            lod: lod_of(*lod),
        },
        Event::TextRequest {
            id,
            text,
            px,
            weight,
            font,
        } => EngineEvent::TextRequest {
            id: *id,
            text: text.clone(),
            px: *px,
            weight: *weight,
            font: font.clone(),
        },
        Event::ImageRequest {
            id,
            asset,
            max_side,
            max_pixels,
        } => EngineEvent::ImageRequest {
            id: *id,
            asset: asset.clone(),
            max_side: *max_side,
            max_pixels: *max_pixels,
        },
        // Kept by the view (`NeedFrame`, `Error`) or not yet used.
        Event::NeedFrame
        | Event::SectionEnter { .. }
        | Event::Announce { .. }
        | Event::Error { .. } => return None,
    })
}

fn object_info(o: &p::ObjectInfo) -> ObjectInfo {
    let p::ObjectInfo {
        label,
        category,
        section,
        row,
        seat,
        flags,
    } = o.clone();
    ObjectInfo {
        label,
        category,
        section,
        row,
        seat,
        flags,
    }
}

fn chart_info(c: &p::ChartInfo) -> ChartInfo {
    let p::ChartInfo {
        name,
        floors,
        sections,
        categories,
        objects,
        seats,
    } = c.clone();
    let floors = floors.into_iter().map(|f| {
        let p::FloorInfo { name, level, seats } = f;
        FloorInfo { name, level, seats }
    });
    let sections = sections.into_iter().map(|s| {
        let p::SectionInfo {
            label,
            floor,
            seats,
        } = s;
        SectionInfo {
            label,
            floor,
            seats,
        }
    });
    let categories = categories.into_iter().map(|k| {
        let p::CategoryInfo {
            key,
            label,
            color,
            objects,
        } = k;
        CategoryInfo {
            key,
            label,
            color,
            objects,
        }
    });
    ChartInfo {
        name,
        floors: floors.collect(),
        sections: sections.collect(),
        categories: categories.collect(),
        objects,
        seats,
    }
}

fn lod_of(lod: p::Lod) -> Lod {
    match lod {
        p::Lod::Overview => Lod::Overview,
        p::Lod::CrossFade => Lod::CrossFade,
        p::Lod::Seats => Lod::Seats,
        p::Lod::Detail => Lod::Detail,
    }
}

/// Both ways, so a theme added on either side does not compile.
fn theme_of(theme: BuiltinTheme) -> ThemeName {
    match theme {
        BuiltinTheme::Light => ThemeName::Light,
        BuiltinTheme::Dark => ThemeName::Dark,
        BuiltinTheme::HighContrast => ThemeName::HighContrast,
    }
}

fn builtin(theme: ThemeName) -> BuiltinTheme {
    match theme {
        ThemeName::Light => BuiltinTheme::Light,
        ThemeName::Dark => BuiltinTheme::Dark,
        ThemeName::HighContrast => BuiltinTheme::HighContrast,
    }
}

fn gestures_of(g: p::GestureHandling) -> GestureHandling {
    match g {
        p::GestureHandling::Greedy => GestureHandling::Greedy,
        p::GestureHandling::Cooperative => GestureHandling::Cooperative,
        p::GestureHandling::None => GestureHandling::None,
    }
}

fn protocol_gestures(g: GestureHandling) -> p::GestureHandling {
    match g {
        GestureHandling::Greedy => p::GestureHandling::Greedy,
        GestureHandling::Cooperative => p::GestureHandling::Cooperative,
        GestureHandling::None => p::GestureHandling::None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::events::{object_with_status, to_json};

    fn object(label: &str) -> p::ObjectInfo {
        p::ObjectInfo {
            label: label.into(),
            category: "a\"b".into(),
            section: "S\\".into(),
            row: "Ä".into(),
            seat: "\n1".into(),
            flags: 9,
        }
    }

    /// Every event, with values JSON and `events.rs` both write exactly.
    fn samples() -> Vec<Event> {
        let lods = [
            p::Lod::Overview,
            p::Lod::CrossFade,
            p::Lod::Seats,
            p::Lod::Detail,
        ];
        let mut events = vec![
            Event::Ready(p::ChartInfo {
                name: "Hall".into(),
                floors: vec![p::FloorInfo {
                    name: "Ground".into(),
                    level: -1,
                    seats: 2,
                }],
                sections: vec![p::SectionInfo {
                    label: "A".into(),
                    floor: 0,
                    seats: 2,
                }],
                categories: vec![p::CategoryInfo {
                    key: "vip".into(),
                    label: "VIP".into(),
                    color: [37, 99, 235, 255],
                    objects: 2,
                }],
                objects: 3,
                seats: 2,
            }),
            Event::HoldRequest {
                request: 7,
                labels: vec!["x".into(), "y".into()],
            },
            Event::ReleaseRequest {
                request: 8,
                labels: vec![],
            },
            Event::SelectionChanged {
                selected: vec![object("A-1")],
                expires_at_ms: Some(1_800_000_000_000),
            },
            Event::SelectionChanged {
                selected: vec![],
                expires_at_ms: None,
            },
            Event::SelectionInvalid {
                rule: "min".into(),
                objects: vec!["A-1".into()],
            },
            Event::SelectionValid,
            Event::ObjectHover(Some((object("A-2"), [-1.5, 640.25]))),
            Event::ObjectHover(None),
            Event::ObjectFocus(Some((object("A-3"), [0.0, 12.0625]))),
            Event::ObjectFocus(None),
            Event::ObjectUnavailable {
                label: "A-4".into(),
            },
            Event::HoldLost {
                labels: vec!["A-5".into()],
            },
            Event::Resync { seq: Some(12) },
            Event::Resync { seq: None },
            Event::TextRequest {
                id: 3,
                text: "Stalls \"Ä\"".into(),
                px: 24,
                weight: 600,
                font: "system-ui, sans-serif".into(),
            },
            Event::ImageRequest {
                id: 4,
                asset: "0123456789abcdef0123456789abcdef".into(),
                max_side: 4096,
                max_pixels: 2_097_152,
            },
            Event::NeedFrame,
            Event::SectionEnter {
                section: "A".into(),
            },
            Event::Announce { text: "hi".into() },
            Event::Error {
                code: p::ErrorCode::Gpu,
                message: "lost".into(),
            },
        ];
        events.extend(lods.map(|lod| Event::CameraChanged {
            centre: [100.5, -3.25],
            zoom: 0.125,
            range: [0.0625, 64.0],
            lod,
        }));
        events
    }

    #[test]
    fn the_encoder_writes_every_event_as_the_schema_says() {
        for event in samples() {
            let mut json = String::new();
            to_json(std::slice::from_ref(&event), &mut json);
            let written: Vec<Value> = serde_json::from_str(&json).unwrap();
            let expected: Vec<Value> = from_protocol(&event)
                .into_iter()
                .map(|e| serde_json::to_value(e).unwrap())
                .collect();
            assert_eq!(written, expected, "{event:?}");
            // And it reads back, nothing unknown or missing.
            for value in written {
                let read: EngineEvent = serde_json::from_value(value).unwrap();
                assert_eq!(Some(read), from_protocol(&event));
            }
        }
    }

    #[test]
    fn the_object_payload_is_as_the_schema_says() {
        let mut json = String::new();
        object_with_status(&mut json, &object("A-1"), 1, [3, 30]);
        let read: ObjectDetails = serde_json::from_str(&json).unwrap();
        let info = object_info(&object("A-1"));
        assert_eq!(
            read,
            ObjectDetails {
                label: info.label,
                category: info.category,
                section: info.section,
                row: info.row,
                seat: info.seat,
                flags: info.flags,
                status: 1,
                place: [3, 30],
            }
        );
    }

    #[test]
    fn option_names_are_the_engines() {
        for theme in [ThemeName::Light, ThemeName::Dark, ThemeName::HighContrast] {
            let name = serde_json::to_value(theme).unwrap();
            let name = name.as_str().unwrap();
            assert_eq!(BuiltinTheme::from_name(name), Some(builtin(theme)));
            assert_eq!(theme_of(builtin(theme)), theme);
        }
        for g in [
            GestureHandling::Greedy,
            GestureHandling::Cooperative,
            GestureHandling::None,
        ] {
            assert_eq!(gestures_of(protocol_gestures(g)), g);
        }
    }
}
