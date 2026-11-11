//! The designer's live scene (`docs/04-renderer.md` §25): one chart for the
//! engine, merged from pieces that are each compiled alone.
//!
//! A piece is the compiled chart of one node of the document being edited
//! (a top-level node, or one child of a section compiled inside a copy of
//! its section), or a section's *shell*: the section with no children and
//! its outline given, which carries the section's fill, label and anchor. The
//! edit core compiles a piece again only when what it was compiled from
//! changes (`cavea-designer`'s scene), so an edit re-expands one node.
//!
//! [`LiveScene::merge`] lays the pieces of the floor shown out one after
//! another in draw order and gives a [`Scene`] that the engine draws exactly
//! as it draws a published chart: same seats, rows, labels, sections, mesh,
//! text and icons, so the same level-of-detail behaviour. The pieces of one
//! section are adjacent (the shell first) and share one section record,
//! whose seats and rows span its children's. Strings are interned once, so a
//! string's index never changes and the dynamic text atlas, keyed by string
//! index, stays valid across merges. Each piece's place in the merged
//! arrays is returned as a [`Span`], for uploads and per-piece transforms.

use std::collections::BTreeMap;
use std::ops::Range;

use cavea_format::{
    Bounds, Chart, Draw, Floor, LabelException, Layer, Mesh, NONE, RenderChunk, Row, SeatLabelRule,
    Section, Segment, Strings, Text,
};

use crate::{label::ICON_KEYS, scene::Scene};

/// One compiled piece of the live scene, its strings interned.
#[derive(Debug, Clone)]
pub struct Piece {
    chart: Chart,
    /// Per string of `chart`: its index in the merged table.
    strings: Vec<u32>,
    /// `chart.label_rules` with merged string indices: each custom list was
    /// copied into the merged table once, whole, since a rule's custom
    /// labels are consecutive strings.
    rules: Vec<SeatLabelRule>,
    /// The floor shown, as an index into `chart.floors`.
    floor: u32,
    role: Role,
}

/// What a piece is, for merging section records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A node outside any section.
    Node,
    /// Section `id` with no children: its outline, fill and label.
    Shell(u32),
    /// A child of section `id`, compiled inside a copy of it.
    Child(u32),
}

impl Role {
    fn section(self) -> Option<u32> {
        match self {
            Self::Node => None,
            Self::Shell(id) | Self::Child(id) => Some(id),
        }
    }
}

impl Piece {
    /// Seats, for budgets and tests.
    pub fn seats(&self) -> usize {
        self.chart.seat_pos.len()
    }

    pub fn chart(&self) -> &Chart {
        &self.chart
    }
}

/// Where one piece's data sits in the merged scene.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    /// Seats (`idx`), rows and render chunks.
    pub seats: Range<u32>,
    pub rows: Range<u32>,
    pub chunks: Range<u32>,
    /// Mesh vertices, and the piece's index ranges: its section's outline
    /// (a shell's), its lower and upper decoration.
    pub vertices: Range<u32>,
    pub outline: Range<u32>,
    pub below: Range<u32>,
    pub above: Range<u32>,
    /// Text objects, and decoration icon instances (the engine draws only
    /// the icons whose key it knows).
    pub texts: Range<u32>,
    pub icons: Range<u32>,
    /// Its merged section, or `NONE`.
    pub section: u32,
}

/// The interned strings of every piece merged so far.
#[derive(Debug, Default)]
pub struct LiveScene {
    strings: Strings,
    index: BTreeMap<String, u32>,
}

impl LiveScene {
    /// Interns `s`; `NONE` past the table's 4 GiB.
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        match self.strings.push(s) {
            Some(i) => {
                self.index.insert(s.to_owned(), i);
                i
            }
            None => NONE,
        }
    }

    /// Strings in the merged table, for tests and memory figures.
    pub fn strings(&self) -> usize {
        self.strings.len()
    }

    /// Takes a compiled chart (which passed [`Chart::check`]) as a piece shown
    /// on its floor `floor`, interning its strings.
    pub fn piece(&mut self, chart: Chart, floor: u32, role: Role) -> Piece {
        let strings = (0..chart.strings.len() as u32)
            .map(|i| self.intern(chart.strings.get(i).unwrap_or_default()))
            .collect::<Vec<_>>();
        let map = |i: u32| strings.get(i as usize).copied().unwrap_or(NONE);
        let rules = chart
            .label_rules
            .iter()
            .map(|r| {
                let first = self.strings.len() as u32;
                let custom = r.custom_first..r.custom_first.saturating_add(r.custom_count);
                for i in custom {
                    // A table past 4 GiB leaves the list short; the label
                    // falls back as the compiler's custom lists do.
                    let _ = self.strings.push(chart.strings.get(i).unwrap_or_default());
                }
                SeatLabelRule {
                    prefix: opt(map, r.prefix),
                    suffix: opt(map, r.suffix),
                    custom_first: first,
                    ..*r
                }
            })
            .collect();
        Piece {
            chart,
            strings,
            rules,
            floor,
            role,
        }
    }

    /// One scene of `pieces`, in draw order, on floor `floor` of `base`: a
    /// compile of the chart-level data alone, which gives the metadata,
    /// categories and the floor shown. The pieces of one section must be
    /// adjacent, its shell first.
    pub fn merge(&mut self, base: &Chart, floor: u32, pieces: &[&Piece]) -> (Scene, Vec<Span>) {
        let base_strings: Vec<u32> = (0..base.strings.len() as u32)
            .map(|i| self.intern(base.strings.get(i).unwrap_or_default()))
            .collect();
        let b = |i: u32| base_strings.get(i as usize).copied().unwrap_or(NONE);
        let mut m = Merge::default();
        for piece in pieces {
            m.piece(piece);
        }
        let Merge {
            mut chart,
            pos,
            mut spans,
            outlines,
            below,
            above,
            bounds,
            ..
        } = m;
        // The index buffer: outlines (one range, the fills), then the lower
        // and upper decoration, each one draw range.
        let (o, lo) = (outlines.len() as u32, below.len() as u32);
        for span in &mut spans {
            span.below = span.below.start + o..span.below.end + o;
            span.above = span.above.start + o + lo..span.above.end + o + lo;
        }
        let mut draws = Vec::new();
        for (layer, first, list) in [(Layer::Below, o, &below), (Layer::Above, o + lo, &above)] {
            if !list.is_empty() {
                draws.push(Draw {
                    floor: 0,
                    layer,
                    index_first: first,
                    index_count: list.len() as u32,
                });
            }
        }
        let mut indices = outlines;
        indices.extend_from_slice(&below);
        indices.extend_from_slice(&above);
        chart.mesh.indices = indices;
        chart.mesh.draws = draws;

        let seats = pos.len() as u32;
        let shown = base.floors.get(floor as usize);
        let bounds = bounds.unwrap_or_default();
        chart.floors = vec![Floor {
            name: shown.map_or(b(base.meta.name), |f| b(f.name)),
            level: shown.map_or(0, |f| f.level),
            seat_first: 0,
            seat_count: seats,
            bounds,
            focal: shown.and_then(|f| f.focal),
        }];
        chart.categories = base
            .categories
            .iter()
            .map(|c| cavea_format::Category {
                key: b(c.key),
                label: b(c.label),
                ..*c
            })
            .collect();
        chart.meta = cavea_format::Meta {
            name: b(base.meta.name),
            label_template: b(base.meta.label_template),
            table_label_template: b(base.meta.table_label_template),
            compiler: b(base.meta.compiler),
            seat_count: seats,
            bounds,
            ..base.meta
        };
        chart.strings = self.strings.clone();
        (Scene::from_chart(chart, pos), spans)
    }
}

fn opt(map: impl Fn(u32) -> u32, i: u32) -> u32 {
    if i == NONE { NONE } else { map(i) }
}

/// The merged tables as they fill.
struct Merge {
    chart: Chart,
    pos: Vec<[f32; 2]>,
    spans: Vec<Span>,
    /// Index lists: section outlines in section order, lower and upper
    /// decoration in piece order.
    outlines: Vec<u32>,
    below: Vec<u32>,
    above: Vec<u32>,
    /// The section being filled: its node and merged index, and its box.
    group: Option<(u32, u32)>,
    group_bounds: Option<Bounds>,
    /// Everything's box, for the floor and the camera.
    bounds: Option<Bounds>,
    /// Decoration icon instances so far: those whose key the engine draws.
    icons: u32,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            chart: Chart {
                meta: cavea_format::Meta {
                    name: 0,
                    label_template: 0,
                    table_label_template: 0,
                    compiler: 0,
                    seat_count: 0,
                    bounds: [0; 4],
                    seat_size: 0,
                    seat_shape: cavea_format::SeatShape::Circle,
                    row_label_display: cavea_format::RowLabelDisplay::Both,
                    rank_algo: 0,
                },
                strings: Strings::default(),
                categories: Vec::new(),
                floors: Vec::new(),
                sections: Vec::new(),
                chunks: Vec::new(),
                rows: Vec::new(),
                segments: Vec::new(),
                label_rules: Vec::new(),
                label_exceptions: Vec::new(),
                seat_pos: Vec::new(),
                seat_category: Vec::new(),
                seat_flags: Vec::new(),
                objects: Vec::new(),
                tables: Vec::new(),
                mesh: Mesh::default(),
                texts: Vec::new(),
                icons: Vec::new(),
                images: Vec::new(),
                full: None,
            },
            pos: Vec::new(),
            spans: Vec::new(),
            outlines: Vec::new(),
            below: Vec::new(),
            above: Vec::new(),
            group: None,
            group_bounds: None,
            bounds: None,
            icons: 0,
        }
    }
}

fn grow(b: &mut Option<Bounds>, [x0, y0, x1, y1]: Bounds) {
    *b = Some(match *b {
        None => [x0, y0, x1, y1],
        Some([a0, b0, a1, b1]) => [a0.min(x0), b0.min(y0), a1.max(x1), b1.max(y1)],
    });
}

fn len(n: usize) -> u32 {
    // Every table is within the format's limits.
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Merge {
    fn piece(&mut self, p: &Piece) {
        let ch = &p.chart;
        let c = &mut self.chart;
        let s = |i: u32| opt(|i| p.strings.get(i as usize).copied().unwrap_or(NONE), i);
        let seat_off = len(self.pos.len());
        let row_off = len(c.rows.len());
        let rule_off = len(c.label_rules.len());
        let chunk_off = len(c.chunks.len());
        let vert_off = len(c.mesh.vertices.len());
        let text_off = len(c.texts.len());
        let icon_off = self.icons;

        // The section record: the first piece of a section's group makes it
        // (the shell, which gives its outline and label, unless the shell
        // compiled to nothing); its children add their seats and rows.
        let record = ch.sections.iter().find(|r| r.floor == p.floor);
        let open = match (self.group, p.role.section()) {
            (Some((g, index)), Some(id)) if g == id => Some(index),
            _ => None,
        };
        let mut outline = 0..0;
        let section = match (open, record) {
            (Some(index), _) => index,
            (None, Some(record)) => {
                let index = len(c.sections.len());
                let shell = matches!(p.role, Role::Shell(_));
                let first = len(self.outlines.len());
                if shell {
                    let range = record.mesh_first as usize
                        ..(record.mesh_first + record.mesh_count) as usize;
                    let indices = ch.mesh.indices.get(range).unwrap_or_default();
                    self.outlines.extend(indices.iter().map(|&i| i + vert_off));
                }
                outline = first..len(self.outlines.len());
                c.sections.push(Section {
                    label: s(record.label),
                    floor: 0,
                    seat_first: seat_off,
                    seat_count: 0,
                    row_first: row_off,
                    row_count: 0,
                    entrance: s(record.entrance),
                    mesh_first: first,
                    mesh_count: outline.end - first,
                    label_size: if shell { record.label_size } else { 0 },
                    ..*record
                });
                // A shell's box is its outline's (it has no seats); a child's
                // own record also holds the stand-in outline it was compiled
                // with, so the group's box grows from its rows instead.
                self.group_bounds = shell.then_some(record.bounds);
                self.group = p.role.section().map(|id| (id, index));
                index
            }
            (None, None) => {
                self.group = None;
                NONE
            }
        };
        let map_section = |i: u32| if i == NONE { NONE } else { section };

        // Seats and their per-seat arrays.
        self.pos.extend(
            ch.seat_pos
                .iter()
                .map(|&[x, y]| [x as f32 * 0.25, y as f32 * 0.25]),
        );
        c.seat_category.extend_from_slice(&ch.seat_category);
        c.seat_flags.extend_from_slice(&ch.seat_flags);
        c.label_exceptions
            .extend(ch.label_exceptions.iter().map(|x| LabelException {
                idx: x.idx + seat_off,
                label: s(x.label),
            }));
        c.label_rules.extend_from_slice(&p.rules);
        let mut rows_box = None;
        c.rows.extend(ch.rows.iter().map(|r| {
            grow(&mut rows_box, r.bounds);
            Row {
                section: map_section(r.section),
                label: s(r.label),
                seat_first: r.seat_first + seat_off,
                label_rule: if r.label_rule == NONE {
                    NONE
                } else {
                    r.label_rule + rule_off
                },
                ..*r
            }
        }));
        c.segments.extend(ch.segments.iter().map(|g| Segment {
            row: g.row + row_off,
            seat_first: g.seat_first + seat_off,
            ..*g
        }));
        c.chunks.extend(ch.chunks.iter().map(|k| RenderChunk {
            section: map_section(k.section),
            seat_first: k.seat_first + seat_off,
            ..*k
        }));
        let seats = len(ch.seat_pos.len());
        let rows = len(ch.rows.len());
        if let Some(b) = rows_box {
            grow(&mut self.bounds, b);
            if section != NONE {
                grow(&mut self.group_bounds, b);
            }
        }
        if let Some(rec) = c.sections.get_mut(section as usize) {
            rec.seat_count += seats;
            rec.row_count += rows;
            rec.bounds = self.group_bounds.unwrap_or_default();
            if let Some(b) = self.group_bounds {
                grow(&mut self.bounds, b);
            }
        }

        // Decoration: its vertices and its index ranges on the floor shown.
        c.mesh.vertices.extend_from_slice(&ch.mesh.vertices);
        let mut deco = None;
        let mut layer = |which: Layer, out: &mut Vec<u32>| {
            let first = len(out.len());
            let draws = ch.mesh.draws.iter();
            for d in draws.filter(|d| d.floor == p.floor && d.layer == which) {
                let range = d.index_first as usize..(d.index_first + d.index_count) as usize;
                for &i in ch.mesh.indices.get(range).unwrap_or_default() {
                    out.push(i + vert_off);
                    if let Some(v) = ch.mesh.vertices.get(i as usize) {
                        let [x, y] = v.pos.map(|q| (q * 4.0) as i32);
                        grow(&mut deco, [x, y, x, y]);
                    }
                }
            }
            first..len(out.len())
        };
        let below = layer(Layer::Below, &mut self.below);
        let above = layer(Layer::Above, &mut self.above);
        let on_floor = |f: u32| f == p.floor;
        for t in ch.texts.iter().filter(|t| on_floor(t.floor)) {
            let [x, y] = t.pos;
            grow(&mut deco, [x, y, x, y]);
            c.texts.push(Text {
                text: s(t.text),
                floor: 0,
                ..*t
            });
        }
        for i in ch.icons.iter().filter(|i| on_floor(i.floor)) {
            let key = ch.strings.get(i.key).unwrap_or_default();
            self.icons += u32::from(ICON_KEYS.contains(&key));
            let [x, y] = i.pos;
            grow(&mut deco, [x, y, x, y]);
            c.icons.push(cavea_format::Icon {
                key: s(i.key),
                floor: 0,
                ..*i
            });
        }
        c.images
            .extend(
                ch.images
                    .iter()
                    .filter(|i| on_floor(i.floor))
                    .map(|i| cavea_format::Image {
                        asset: s(i.asset),
                        floor: 0,
                        ..*i
                    }),
            );
        if let Some(b) = deco {
            grow(&mut self.bounds, b);
        }
        self.spans.push(Span {
            seats: seat_off..seat_off + seats,
            rows: row_off..row_off + rows,
            chunks: chunk_off..len(c.chunks.len()),
            vertices: vert_off..len(c.mesh.vertices.len()),
            outline,
            below,
            above,
            texts: text_off..len(c.texts.len()),
            icons: icon_off..self.icons,
            section,
        });
    }
}
