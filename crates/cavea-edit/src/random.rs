//! Seeded random edits for property tests: the generator behind the edit
//! core's own tests (`command_inverses` and the panel and shell tests),
//! exported with the `random` feature so that the designer module's live
//! scene can be tested against the same edit sequences. Not used in
//! production builds.

#![allow(
    clippy::expect_used,
    reason = "a test generator: the node JSON it parses is fixed"
)]

use cavea_compiler::document::{Category, Curve, Floor, Node, SeatFlag, SeatOverride};

use crate::{Axis, ChartProperty, Command, Document, End, NodeTransform, Property, SeatEdit};

/// A seeded SplitMix64 generator.
pub struct Rng(pub u64);

impl Rng {
    #[allow(
        clippy::should_implement_trait,
        reason = "an infinite generator, not an iterator"
    )]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub fn f(&mut self, range: f64) -> f64 {
        (self.next() % 2001) as f64 / 1000.0 * range - range
    }

    pub fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        items.get(self.below(items.len())).copied()
    }
}

fn node(text: &str) -> Box<Node> {
    Box::new(serde_json::from_str(text).expect("node"))
}

fn blocks(doc: &Document) -> Vec<u32> {
    doc.ids()
        .filter(|&id| matches!(doc.node(id), Some(Node::RowBlock(_))))
        .collect()
}

/// A random command against the document: usually valid, sometimes not.
pub fn command(r: &mut Rng, doc: &Document) -> Command {
    let all: Vec<u32> = doc.ids().collect();
    let sections: Vec<u32> = doc
        .children(None)
        .iter()
        .copied()
        .filter(|&id| matches!(doc.node(id), Some(Node::Section(_))))
        .collect();
    let blocks = blocks(doc);
    let id = r.pick(&all).unwrap_or(1);
    let delta = [r.f(500.0), r.f(500.0)];
    let parent = if r.below(3) == 0 {
        None
    } else {
        r.pick(&sections)
    };
    let index = r.below(doc.children(parent).len() + 2) as u32;
    let block = r.pick(&blocks).and_then(|b| match doc.node(b) {
        Some(Node::RowBlock(b)) => Some(b.clone()),
        _ => None,
    });
    let rid = block
        .as_ref()
        .and_then(|b| b.rows.get(r.below(b.rows.len())).map(|row| row.rid))
        .unwrap_or(7);
    let oid = block
        .as_ref()
        .and_then(|b| b.rows.iter().find(|row| row.rid == rid))
        .and_then(|row| {
            row.oids
                .first()
                .map(|o| o[0] + r.below(o[1] as usize) as u32)
        })
        .unwrap_or(9);
    let bid = block.as_ref().map_or(id, |b| b.id);
    let end = if r.below(2) == 0 {
        End::Start
    } else {
        End::End
    };
    let cat = |r: &mut Rng| ["standard", "vip", "access", "premium", "x"][r.below(5)].to_string();
    match r.below(30) {
        0 | 1 => {
            let text = match r.below(4) {
                0 => {
                    r#"{"id":0,"type":"rowBlock","rows":[{"rid":0,"seats":6,"oids":[[50,6]]},
                    {"rid":0,"seats":4,"oids":[[60,2],[70,2]],"removed":[1]}],
                    "seatOverrides":{"51":{"flags":["aisle"]},"71":{"label":"Q"}}}"#
                }
                1 => r#"{"id":0,"type":"shape","kind":"rect","size":[100,40]}"#,
                2 => r##"{"id":0,"type":"text","text":"STAGE","size":80,"color":"#FFFFFF"}"##,
                _ => {
                    r#"{"id":0,"type":"section","label":"New","children":[
                    {"id":0,"type":"rowBlock","rows":[{"rid":0,"seats":3,"oids":[[1,3]]}]}]}"#
                }
            };
            let mut node = node(text);
            match &mut *node {
                Node::RowBlock(b) => b.origin = delta,
                Node::Shape(s) => s.origin = delta,
                Node::Text(t) => t.origin = delta,
                _ => {}
            }
            Command::CreateNode {
                parent,
                index,
                node,
            }
        }
        2 => Command::DeleteNodes {
            ids: vec![id, r.pick(&all).unwrap_or(id)],
        },
        3 | 4 => Command::MoveNodes {
            ids: vec![id, r.pick(&all).unwrap_or(id)],
            delta,
        },
        5 => Command::TransformNodes {
            nodes: vec![NodeTransform {
                id,
                origin: delta,
                rotation: r.f(180.0),
            }],
        },
        6 => Command::PlaceNode { id, parent, index },
        7 | 8 => {
            let property = match r.below(10) {
                0 => Property::SeatPitch(40.0 + r.f(10.0)),
                1 => Property::Curve(Curve::Arc {
                    radius: 2000.0 + r.f(500.0),
                }),
                2 => Property::Label(Some("L".into())),
                3 => Property::Category(Some(cat(r))),
                4 => Property::Name(None),
                5 => Property::Hidden(r.below(2) == 0),
                6 => Property::Priority(r.f(2.0)),
                7 => Property::Stagger(r.f(0.5)),
                8 => Property::Decoration(node(
                    r##"{"id":5,"type":"text","text":"EXIT","size":40,"color":"#000000"}"##,
                )),
                _ => Property::Skew(r.f(20.0)),
            };
            let id = if r.below(2) == 0 { bid } else { id };
            Command::SetProperty { id, property }
        }
        9 => Command::SetChart {
            property: ChartProperty::Name(format!("Chart {}", r.below(100))),
        },
        10 => Command::AddCategory {
            index: 0,
            category: Category {
                key: cat(r),
                label: "C".into(),
                color: "#123456".into(),
                accessible: false,
            },
        },
        11 => {
            let key = doc
                .categories()
                .first()
                .map_or("vip".into(), |c| c.key.clone());
            match r.below(2) {
                0 => Command::RemoveCategory {
                    key,
                    replacement: (r.below(2) == 0).then(|| cat(r)),
                },
                _ => Command::UpdateCategory {
                    key,
                    category: Category {
                        key: cat(r),
                        label: "Renamed".into(),
                        color: "#654321".into(),
                        accessible: true,
                    },
                },
            }
        }
        12 => match r.below(3) {
            0 => Command::AddFloor {
                index: 0,
                floor: Floor {
                    id: 0,
                    name: "Upper".into(),
                    level: 1,
                    focal_point: None,
                },
            },
            1 => Command::RemoveFloor {
                id: doc.floors().first().map_or(1, |f| f.id),
                replacement: doc.floors().last().map(|f| f.id),
            },
            _ => Command::SetProperty {
                id,
                property: Property::Floor(doc.floors().first().map(|f| f.id)),
            },
        },
        13 => Command::AddSeats {
            id: bid,
            rid,
            end,
            count: 1 + r.below(4) as u32,
        },
        14 => Command::ResizeRows {
            id: bid,
            end,
            count: r.below(7) as i32 - 3,
        },
        15 | 16 => Command::RemoveSeats {
            id: bid,
            rid,
            end,
            count: 1 + r.below(3) as u32,
        },
        17 => Command::AddRows {
            id: bid,
            index: r.below(4) as u32,
            count: 1 + r.below(2) as u32,
            seats: 1 + r.below(12) as u32,
        },
        18 => Command::RemoveRows {
            id: bid,
            rids: vec![rid],
        },
        19 => Command::RemoveSeat { id: bid, oid },
        20 => Command::RestoreSeat { id: bid, oid },
        21 => Command::SetSeatOverride {
            id: bid,
            oid,
            value: (r.below(3) > 0).then(|| SeatOverride {
                flags: Vec::new(),
                category: Some(cat(r)),
                label: None,
                nudge: Some([r.f(5.0), 0.0]),
            }),
        },
        22 => Command::MoveNodes {
            ids: vec![id],
            delta: [f64::MAX, 0.0],
        },
        24 | 25 => {
            let edit = match r.below(7) {
                0 => SeatEdit::Remove,
                1 => SeatEdit::Restore,
                2 => SeatEdit::Flag {
                    flag: [SeatFlag::Accessible, SeatFlag::Aisle][r.below(2)],
                    on: r.below(3) > 0,
                },
                3 => SeatEdit::Category {
                    category: (r.below(3) > 0).then(|| cat(r)),
                },
                4 => SeatEdit::Label {
                    label: (r.below(2) == 0).then(|| "W".repeat(r.below(70))),
                },
                5 => SeatEdit::Nudge {
                    by: [r.f(5.0), r.f(5.0)],
                },
                _ => SeatEdit::Nudge {
                    by: [f64::MAX, f64::MAX],
                },
            };
            let row = block
                .as_ref()
                .and_then(|b| b.rows.iter().find(|row| row.rid == rid));
            let oids = row
                .and_then(|row| row.oids.first())
                .map_or(vec![oid], |o| (o[0]..o[0] + o[1].min(5)).collect());
            Command::EditSeats {
                id: bid,
                oids,
                edit,
            }
        }
        26 => Command::ShiftRows {
            id: bid,
            rids: vec![rid],
            delta: [27.5, -55.0, f64::MAX][r.below(3)],
        },
        27 => Command::SetRowLabel {
            id: bid,
            rid,
            label: (r.below(2) == 0).then(|| "AA".into()),
        },
        23 => Command::MoveCategory {
            key: doc
                .categories()
                .last()
                .map_or("vip".into(), |c| c.key.clone()),
            index: r.below(doc.categories().len() + 1) as u32,
        },
        28 => Command::InsertGap {
            id: bid,
            rid,
            at: r.below(6) as u32,
            count: r.below(3) as u32,
        },
        29 => Command::FlipNodes {
            ids: vec![id, r.pick(&all).unwrap_or(id)],
            axis: [Axis::Horizontal, Axis::Vertical][r.below(2)],
            about: r.f(300.0),
        },
        _ => Command::Composite {
            commands: vec![command(r, doc), command(r, doc)],
        },
    }
}
