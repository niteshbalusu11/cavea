//! The compiler pipeline for seated charts (`docs/03-chart-format.md` §3.1):
//! chart document in, public and full CCF artefacts out.
//!
//! [`compile`] runs, in order: the schema and structure checks the pipeline
//! needs (steps 1–2), expansion (3), the transform to world coordinates (4),
//! row segmentation (5), labelling and the uniqueness check (6–7), ranking (8),
//! `idx` order and render chunks (9–10), section outlines, label anchors and
//! outline tessellation (11–12, in [`crate::geometry`] and
//! [`crate::tessellate`]), decoration shapes ([`decoration`]), quantisation
//! (13), and encoding with the content hash (14–15). A chart without errors
//! is then checked for the warnings of 03 §3.2 ([`warnings`]).
//!
//! Everything here is deterministic: no platform maths beyond `+ − × ÷ √`,
//! no hash-map iteration, and every sort has a total order.

use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::ops::Range;

use cavea_core::label::{LabelBuf, Scheme, SeatDirection};
use cavea_core::limits::{MAX_CATEGORIES, MAX_FLOORS, MAX_SECTIONS};
use cavea_format::{self as ccf, Artefact, Bounds, CHUNK_MAX_SEATS, NONE, Point};
use lyon_tessellation::FillRule;

use crate::SCHEMA;
use crate::detmath;
use crate::document::{self as doc, ChartDocument, Node};
use crate::expand::{ExpandedRow, Expansion, Seat};
use crate::finding::{Code, Detail, Finding, Findings, expand_code};
use crate::geometry::{self, OutlineError, P};
use crate::tessellate::{self, TessError};

mod decoration;
mod warnings;

use decoration::{Decorated, Decoration};

/// The seat rank formula of 03 §3.3, stored in `META`.
pub const RANK_ALGO: u8 = 1;

/// Both artefacts, their content hashes and statistics.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub public: Vec<u8>,
    pub full: Vec<u8>,
    pub public_hash: [u8; 16],
    pub full_hash: [u8; 16],
    pub stats: Stats,
    /// The warnings (03 §3.2), in the order of 03 §3.2. Errors stop the
    /// compile and come back in [`CompileError::Invalid`] instead.
    pub findings: Vec<Finding>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub seats: u32,
    pub rows: u32,
    pub segments: u32,
    pub sections: u32,
    pub floors: u32,
    pub chunks: u32,
    pub label_rules: u32,
    pub label_exceptions: u32,
    pub public_bytes: usize,
    pub full_bytes: usize,
}

#[derive(Debug)]
pub enum CompileError {
    /// The document has errors; nothing was compiled. At most
    /// [`crate::finding::MAX_FINDINGS`] are listed, all errors: warnings are
    /// checked only on a chart without errors.
    Invalid(Vec<Finding>),
    /// The chart could not be encoded: more than 4 GiB of output, or a
    /// compiler bug (the chart failed `cavea_format::Chart::check`).
    Encode(ccf::Error),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(findings) => {
                f.write_str("chart document has errors")?;
                findings.iter().try_for_each(|x| write!(f, "\n  {x}"))
            }
            Self::Encode(e) => write!(f, "compiled chart: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// Writes an object's full label: `template` with `{section}`, `{row}` and
/// `{seat}` replaced by the parts, in one pass (03 §3.1 step 6). Any other text,
/// braces included, is copied as is. Fails past 64 bytes when `out` is a
/// [`LabelBuf`]. A consumer of a compiled chart composes labels the same way
/// from `META`'s template, the section and row labels, and the seat label.
pub fn write_full_label<W: Write>(
    template: &str,
    section: &str,
    row: &str,
    seat: &str,
    out: &mut W,
) -> fmt::Result {
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (text, tail) = rest.split_at(open);
        out.write_str(text)?;
        let part = [("{section}", section), ("{row}", row), ("{seat}", seat)]
            .into_iter()
            .find(|(key, _)| tail.starts_with(key));
        let (len, value) = match part {
            Some((key, value)) => (key.len(), value),
            None => (1, "{"),
        };
        out.write_str(value)?;
        rest = tail.get(len..).unwrap_or_default();
    }
    out.write_str(rest)
}

/// Compiles a chart document to its two artefacts and its warnings. The same
/// document always gives the same bytes and the same findings.
pub fn compile(document: &ChartDocument) -> Result<Compiled, CompileError> {
    let (chart, findings) = check(document)?;
    emit(&chart, findings)
}

/// Steps 1–13 and the warnings: everything but encoding.
fn check(document: &ChartDocument) -> Result<(ccf::Chart, Vec<Finding>), CompileError> {
    let mut f = Findings::default();
    if document.schema != SCHEMA {
        f.push(
            Code::E010,
            None,
            Detail::Schema {
                schema: document.schema,
            },
        );
        return Err(CompileError::Invalid(f.0));
    }
    let mut c = Compiler::new(document);
    c.categories(&mut f);
    c.floors(&mut f);
    let mut units = c.plan(&mut f);
    // `idx` order (02 §3): by floor; on each floor its sections, then rows
    // outside any section; each in document order.
    units.sort_by_key(|u| (u.floor, u.section.is_none(), u.order));
    let expansion = c.expand(&mut units, &mut f);
    let chart = c.build(&units, &expansion, &mut f);
    if f.has_errors() {
        return Err(CompileError::Invalid(f.0));
    }
    let too_large = ccf::Error {
        chunk: Some(ccf::tags::STRS),
        kind: ccf::ErrorKind::TooLarge,
    };
    let chart = chart.ok_or(CompileError::Encode(too_large))?;
    Ok((chart, f.0))
}

/// Encodes both artefacts and writes their content hashes (03 §4.1).
fn emit(chart: &ccf::Chart, findings: Vec<Finding>) -> Result<Compiled, CompileError> {
    let artefact = |which| -> Result<(Vec<u8>, [u8; 16]), CompileError> {
        let mut bytes = ccf::encode(chart, which).map_err(CompileError::Encode)?;
        let digest = blake3::hash(bytes.get(ccf::HEADER_LEN..).unwrap_or_default());
        let mut hash = [0u8; 16];
        hash.copy_from_slice(digest.as_bytes().get(..16).unwrap_or(&[0; 16]));
        ccf::set_content_hash(&mut bytes, hash);
        Ok((bytes, hash))
    };
    let (public, public_hash) = artefact(Artefact::Public)?;
    let (full, full_hash) = artefact(Artefact::Full)?;
    let len = |n: usize| n as u32; // every table is within MAX_OBJECTS, checked by encode
    let stats = Stats {
        seats: chart.meta.seat_count,
        rows: len(chart.rows.len()),
        segments: len(chart.segments.len()),
        sections: len(chart.sections.len()),
        floors: len(chart.floors.len()),
        chunks: len(chart.chunks.len()),
        label_rules: len(chart.label_rules.len()),
        label_exceptions: len(chart.label_exceptions.len()),
        public_bytes: public.len(),
        full_bytes: full.len(),
    };
    Ok(Compiled {
        public,
        full,
        public_hash,
        full_hash,
        stats,
        findings,
    })
}

/// Rotation by `rotation` degrees clockwise, then translation by `origin`.
#[derive(Clone, Copy, Debug)]
struct Place {
    sin: f64,
    cos: f64,
    origin: [f64; 2],
}

impl Place {
    const IDENTITY: Self = Self {
        sin: 0.0,
        cos: 1.0,
        origin: [0.0, 0.0],
    };

    fn new(origin: [f64; 2], rotation: f64) -> Result<Self, &'static str> {
        if !origin.iter().all(|v| v.is_finite()) {
            return Err("origin");
        }
        if !rotation.is_finite() {
            return Err("rotation");
        }
        // Whole turns first: `sin_cos` is accurate only for moderate angles.
        // `%` is C's `fmod`, whose result is exact, so every target agrees.
        let (sin, cos) = detmath::sin_cos(detmath::radians(rotation % 360.0));
        Ok(Self { sin, cos, origin })
    }

    /// With y down, this matrix turns +x towards +y: clockwise on screen.
    fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [
            x * self.cos - y * self.sin + self.origin[0],
            x * self.sin + y * self.cos + self.origin[1],
        ]
    }
}

/// Quantises chart units to quarter units, rounding half to even (03 §1).
/// `None` unless `|4x| ≤ 2³¹ − 1`.
fn quantise(x: f64) -> Option<i32> {
    const TOINT: f64 = 1.5 / f64::EPSILON; // 1.5 × 2⁵²
    let v = x * 4.0;
    // Also rejects NaN and infinities.
    if v.is_nan() || v.abs() > f64::from(i32::MAX) {
        return None;
    }
    // Adding and subtracting 1.5 × 2⁵² rounds to an integer, ties to even, in
    // the default rounding mode, which Rust never changes.
    Some(((v + TOINT) - TOINT) as i32)
}

fn quantise_point([x, y]: [f64; 2]) -> Option<Point> {
    Some([quantise(x)?, quantise(y)?])
}

fn grow_box(b: &mut Bounds, other: Bounds) {
    let mut grown = Some(*b);
    grow(&mut grown, other);
    *b = grown.unwrap_or(other);
}

/// Grows `b` to include `p`.
fn grow(b: &mut Option<Bounds>, [x0, y0, x1, y1]: Bounds) {
    *b = Some(match *b {
        None => [x0, y0, x1, y1],
        Some([a0, b0, a1, b1]) => [a0.min(x0), b0.min(y0), a1.max(x1), b1.max(y1)],
    });
}

fn point_box([x, y]: Point) -> Bounds {
    [x, y, x, y]
}

/// A quantised box that contains every point of `pts`: minima rounded down
/// and maxima up to quarter units. `None` out of range.
fn outer_box(pts: &[[f64; 2]]) -> Option<Bounds> {
    let q = |v: f64, up: bool| {
        let v = if up {
            (v * 4.0).ceil()
        } else {
            (v * 4.0).floor()
        };
        quantise(v / 4.0)
    };
    let mut b: Option<Bounds> = None;
    for &[x, y] in pts {
        grow(
            &mut b,
            [q(x, false)?, q(y, false)?, q(x, true)?, q(y, true)?],
        );
    }
    b
}

/// Section outline vertices are white: the renderer tints them (03 §4.2.3).
const OUTLINE_COLOR: ccf::Rgba = [255, 255, 255, 255];
/// Half the default row pitch: the outline padding of a section without row
/// blocks.
const DEFAULT_PADDING: f64 = 47.5;

/// A bookable node to expand, and where it sits in the world.
struct Member<'d> {
    node: Bookable<'d>,
    /// The node's own placement, then its section's.
    place: [Place; 2],
    /// Expanded rows, filled by [`Compiler::expand`].
    rows: Range<usize>,
}

#[derive(Clone, Copy)]
enum Bookable<'d> {
    Block(&'d doc::RowBlock),
    Row(&'d doc::Row),
}

impl Bookable<'_> {
    fn id(self) -> u32 {
        match self {
            Self::Block(b) => b.id,
            Self::Row(r) => r.id,
        }
    }
}

/// A section, or one node outside any section: a run of `idx` order.
struct Unit<'d> {
    floor: usize,
    section: Option<&'d doc::Section>,
    /// Position among the document's top-level nodes.
    order: usize,
    members: Vec<Member<'d>>,
    /// The section's placement in the world; identity outside sections.
    place: Place,
}

struct FloorInfo<'d> {
    name: &'d str,
    level: i32,
    focal: Option<Point>,
}

/// String table with each distinct string stored once, in first-use order.
#[derive(Default)]
struct Interner {
    strings: ccf::Strings,
    map: BTreeMap<String, u32>,
    overflow: bool,
}

impl Interner {
    fn get(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let Some(i) = self.strings.push(s) else {
            self.overflow = true;
            return 0;
        };
        self.map.insert(s.to_owned(), i);
        i
    }

    fn opt(&mut self, s: Option<&str>) -> u32 {
        s.map_or(NONE, |s| self.get(s))
    }
}

/// A seat label rule as an ordered key, for storing each distinct rule once.
type RuleKey<'d> = (u8, u8, u32, u32, &'d str, &'d str, &'d [String]);

struct Compiler<'d> {
    doc: &'d ChartDocument,
    strings: Interner,
    categories: Vec<ccf::Category>,
    category_index: BTreeMap<&'d str, u16>,
    floors: Vec<FloorInfo<'d>>,
    floor_index: BTreeMap<u32, usize>,
    /// Every `id`, `rid` and `oid` with the node it belongs to.
    ids: Vec<(u32, u32)>,
    /// Visible decoration nodes in document order.
    decorations: Vec<Decoration<'d>>,
    /// Decoration nodes met so far, by kind (shape, text, icon, image).
    deco_counts: [usize; 4],
}

impl<'d> Compiler<'d> {
    fn new(doc: &'d ChartDocument) -> Self {
        Self {
            doc,
            strings: Interner::default(),
            categories: Vec::new(),
            category_index: BTreeMap::new(),
            floors: Vec::new(),
            floor_index: BTreeMap::new(),
            ids: Vec::new(),
            decorations: Vec::new(),
            deco_counts: [0; 4],
        }
    }

    fn categories(&mut self, f: &mut Findings) {
        let cats = &self.doc.categories;
        if cats.len() > MAX_CATEGORIES as usize {
            let detail = Detail::TooMany {
                what: "categories",
                limit: MAX_CATEGORIES,
            };
            f.push(Code::E006, None, detail);
            return;
        }
        for c in cats {
            let index = self.categories.len() as u16; // ≤ MAX_CATEGORIES
            if self.category_index.insert(&c.key, index).is_some() {
                let key = c.key.clone();
                f.push(Code::E008, None, Detail::DuplicateCategory { key });
                continue;
            }
            let color = parse_color(&c.color).unwrap_or_else(|| {
                let key = c.key.clone();
                f.push(Code::E005, None, Detail::CategoryColor { key });
                [0; 4]
            });
            self.categories.push(ccf::Category {
                key: self.strings.get(&c.key),
                label: self.strings.get(&c.label),
                color,
                flags: u8::from(c.accessible),
            });
        }
    }

    /// Floors in document order. With none listed, one implicit floor.
    fn floors(&mut self, f: &mut Findings) {
        if self.doc.floors.len() > MAX_FLOORS as usize {
            let detail = Detail::TooMany {
                what: "floors",
                limit: MAX_FLOORS,
            };
            f.push(Code::E006, None, detail);
            return;
        }
        for fl in &self.doc.floors {
            if self.floor_index.insert(fl.id, self.floors.len()).is_some() {
                f.push(Code::E007, None, Detail::DuplicateFloor { floor: fl.id });
            }
            let focal = fl.focal_point.map(|p| {
                quantise_point(p).unwrap_or_else(|| {
                    f.push(Code::E005, None, Detail::FocalPoint { floor: fl.id });
                    [0, 0]
                })
            });
            self.floors.push(FloorInfo {
                name: &fl.name,
                level: fl.level,
                focal,
            });
        }
        if self.floors.is_empty() {
            self.floors.push(FloorInfo {
                name: "",
                level: 0,
                focal: None,
            });
        }
    }

    /// The floor index for a node's `floor`; without one, the first floor.
    fn floor(&self, floor: Option<u32>, node: u32, f: &mut Findings) -> Option<usize> {
        let Some(id) = floor else {
            return Some(0);
        };
        let found = self.floor_index.get(&id).copied();
        if found.is_none() {
            f.push(Code::E007, Some(node), Detail::UnknownFloor { floor: id });
        }
        found
    }

    fn check_category(&self, key: Option<&str>, node: u32, f: &mut Findings) {
        if let Some(key) = key
            && !self.category_index.contains_key(key)
        {
            let key = key.to_owned();
            f.push(Code::E007, Some(node), Detail::UnknownCategory { key });
        }
    }

    /// Walks the visible nodes: collects sections and bookable nodes as units,
    /// node ids, and the structural findings that need no expansion.
    fn plan(&mut self, f: &mut Findings) -> Vec<Unit<'d>> {
        let mut units = Vec::new();
        let mut sections = 0u32;
        for (order, node) in self.doc.nodes.iter().enumerate() {
            let id = node_id(node);
            if is_hidden(node) {
                continue;
            }
            self.ids.push((id, id));
            match node {
                Node::Section(s) => {
                    sections += 1;
                    if let Some(unit) = self.section(s, order, f) {
                        units.push(unit);
                    }
                }
                Node::RowBlock(_) | Node::Row(_) => {
                    let floor = self.floor(node_floor(node), id, f);
                    let member = self.member(node, Place::IDENTITY, f);
                    if let (Some(floor), Some(member)) = (floor, member) {
                        units.push(Unit {
                            floor,
                            section: None,
                            order,
                            members: vec![member],
                            place: Place::IDENTITY,
                        });
                    }
                }
                Node::Shape(_) | Node::Text(_) | Node::Icon(_) | Node::Image(_) => {
                    if let Some(floor) = self.floor(node_floor(node), id, f) {
                        self.decoration(node, (Place::IDENTITY, 0.0), floor, f);
                    }
                }
                other => unsupported(other, f),
            }
        }
        if sections > MAX_SECTIONS {
            let detail = Detail::TooMany {
                what: "sections",
                limit: MAX_SECTIONS,
            };
            f.push(Code::E006, None, detail);
        }
        units
    }

    fn section(&mut self, s: &'d doc::Section, order: usize, f: &mut Findings) -> Option<Unit<'d>> {
        let floor = self.floor(s.floor, s.id, f);
        self.check_category(s.category.as_deref(), s.id, f);
        let priority_ok =
            s.priority.is_finite() && s.priority > 0.0 && (s.priority as f32).is_finite();
        if !priority_ok {
            let detail = Detail::InvalidValue { field: "priority" };
            f.push(Code::E005, Some(s.id), detail);
        }
        let place = Place::new(s.origin, s.rotation)
            .map_err(|field| f.push(Code::E005, Some(s.id), Detail::InvalidValue { field }))
            .ok();
        let mut members = Vec::new();
        for child in s.children.iter().filter(|c| !is_hidden(c)) {
            let id = node_id(child);
            self.ids.push((id, id));
            // A child is on its section's floor; naming another is an error.
            if let Some(own) = node_floor(child)
                && let Some(mine) = self.floor(Some(own), id, f)
                && floor.is_some_and(|sf| sf != mine)
            {
                let section_floor = s.floor.unwrap_or(self.default_floor_id());
                let detail = Detail::FloorConflict {
                    floor: own,
                    section_floor,
                };
                f.push(Code::E007, Some(id), detail);
            }
            match child {
                Node::Shape(_) | Node::Text(_) | Node::Icon(_) | Node::Image(_) => {
                    if let (Some(place), Some(floor)) = (place, floor) {
                        self.decoration(child, (place, s.rotation), floor, f);
                    }
                }
                Node::RowBlock(_) | Node::Row(_) => {
                    if let Some(place) = place
                        && let Some(m) = self.member(child, place, f)
                    {
                        members.push(m);
                    }
                }
                Node::Section(_) => {
                    let detail = Detail::UnsupportedNode {
                        kind: "nested section",
                    };
                    f.push(Code::Unsupported, Some(id), detail);
                }
                other => unsupported(other, f),
            }
        }
        Some(Unit {
            floor: floor?,
            section: Some(s),
            order,
            members,
            place: place?,
        })
    }

    /// The id of the floor a node without `floor` goes on, for comparisons.
    fn default_floor_id(&self) -> u32 {
        self.doc.floors.first().map_or(0, |fl| fl.id)
    }

    fn member(&mut self, node: &'d Node, section: Place, f: &mut Findings) -> Option<Member<'d>> {
        let (id, own, category, overrides, bookable) = match node {
            Node::RowBlock(b) => {
                self.ids.extend(b.rows.iter().map(|r| (r.rid, b.id)));
                let own = Place::new(b.origin, b.rotation);
                (
                    b.id,
                    own,
                    &b.category,
                    &b.seat_overrides,
                    Bookable::Block(b),
                )
            }
            Node::Row(r) => {
                let own = Place::new(r.origin, r.rotation);
                (r.id, own, &r.category, &r.seat_overrides, Bookable::Row(r))
            }
            _ => return None,
        };
        self.check_category(category.as_deref(), id, f);
        for over in overrides.values() {
            self.check_category(over.category.as_deref(), id, f);
        }
        match own {
            Ok(own) => Some(Member {
                node: bookable,
                place: [own, section],
                rows: 0..0,
            }),
            Err(field) => {
                f.push(Code::E005, Some(id), Detail::InvalidValue { field });
                None
            }
        }
    }

    /// Expands every member in unit order, so expanded rows are in `idx` order.
    fn expand(&mut self, units: &mut [Unit<'d>], f: &mut Findings) -> Expansion<'d> {
        let overflow = self.doc.settings.alpha_overflow;
        let mut e = Expansion::new();
        for m in units.iter_mut().flat_map(|u| u.members.iter_mut()) {
            let start = e.rows.len();
            let result = match m.node {
                Bookable::Block(b) => e.push_row_block(b, overflow),
                Bookable::Row(r) => e.push_row(r, overflow),
            };
            if let Err(err) = result {
                f.push(
                    expand_code(&err.kind),
                    Some(err.node),
                    Detail::Expand(err.kind),
                );
            }
            m.rows = start..e.rows.len();
        }
        let rows = &e.rows;
        self.ids.extend(
            e.seats
                .iter()
                .map(|s| (s.oid, rows.get(s.row as usize).map_or(0, |r| r.node))),
        );
        e
    }

    /// Reports every id used twice (E009). Sorting keeps it O(n log n).
    fn check_ids(&mut self, f: &mut Findings) {
        self.ids.sort_unstable();
        for pair in self.ids.windows(2) {
            if let [(a, first), (b, node)] = *pair
                && a == b
            {
                f.push(Code::E009, Some(node), Detail::DuplicateId { id: a, first });
            }
        }
    }

    /// Steps 4–10 and 13: fills the compiled chart. `None` if the string table
    /// overflows. With errors in `f` the result is not consistent and is not
    /// encoded.
    fn build(
        &mut self,
        units: &[Unit<'d>],
        e: &Expansion<'d>,
        f: &mut Findings,
    ) -> Option<ccf::Chart> {
        self.check_ids(f);
        let settings = &self.doc.settings;
        let seat_size = quantise(settings.seat_size)
            .and_then(|q| u16::try_from(q).ok())
            .filter(|&q| q > 0)
            .unwrap_or_else(|| {
                let detail = Detail::InvalidValue { field: "seatSize" };
                f.push(Code::E005, None, detail);
                0
            });
        let shape = match settings.seat_shape {
            doc::SeatShape::Circle => ccf::SeatShape::Circle,
            doc::SeatShape::Rounded => ccf::SeatShape::Rounded,
            doc::SeatShape::Chair => ccf::SeatShape::Chair,
        };
        let mut b = Builder {
            template: &settings.label_template,
            seat_size,
            shape,
            ..Builder::default()
        };
        b.floor_seats = vec![(0, None); self.floors.len()];
        b.expanded_section = vec![None; e.rows.len()];
        for unit in units {
            b.unit(self, unit, e, f);
        }
        b.close_chunk();
        b.check_labels(e, f);

        // Decoration, after the section outlines in `MESH`.
        let mut d = Decorated::new(self.floors.len(), b.outline_indices.len());
        for deco in std::mem::take(&mut self.decorations) {
            if let Err((code, detail)) = d.node(&deco, &mut b.vertices, &mut self.strings) {
                f.push(code, Some(node_id(deco.node)), detail);
            }
        }
        let draws = d.draws(&mut b.outline_indices);
        // A floor's bounds cover its seats, its sections' outlines and its
        // decoration; the chart's cover its floors.
        for s in &b.sections {
            if let Some(fl) = b.floor_seats.get_mut(s.floor as usize)
                && (s.seat_count > 0 || s.mesh_count > 0)
            {
                grow(&mut fl.1, s.bounds);
            }
        }
        let mut chart_bounds = None;
        let mut first = 0u32;
        let floors = self
            .floors
            .iter()
            .zip(&b.floor_seats)
            .zip(&d.bounds)
            .map(|((fl, &(count, mut bounds)), deco)| {
                if let Some(deco) = *deco {
                    grow(&mut bounds, deco);
                }
                if let Some(bounds) = bounds {
                    grow(&mut chart_bounds, bounds);
                }
                let floor = ccf::Floor {
                    name: self.strings.get(fl.name),
                    level: fl.level,
                    seat_first: first,
                    seat_count: count,
                    bounds: bounds.unwrap_or_default(),
                    focal: fl.focal,
                };
                first += count;
                floor
            })
            .collect::<Vec<_>>();
        let ranks = rank(&self.floors, &b.rows, &b.row_rank, &b.seat_pos);
        let meta = ccf::Meta {
            name: self.strings.get(&self.doc.name),
            label_template: self.strings.get(&settings.label_template),
            table_label_template: self.strings.get(&settings.table_label_template),
            compiler: self
                .strings
                .get(concat!("cavea-compiler ", env!("CARGO_PKG_VERSION"))),
            seat_count: b.seat_pos.len() as u32, // ≤ MAX_OBJECTS, checked by expansion
            bounds: chart_bounds.unwrap_or_default(),
            seat_size,
            seat_shape: shape,
            row_label_display: match settings.row_label_display {
                doc::RowLabelDisplay::None => ccf::RowLabelDisplay::None,
                doc::RowLabelDisplay::Start => ccf::RowLabelDisplay::Start,
                doc::RowLabelDisplay::End => ccf::RowLabelDisplay::End,
                doc::RowLabelDisplay::Both => ccf::RowLabelDisplay::Both,
            },
            rank_algo: RANK_ALGO,
        };
        if self.strings.overflow {
            return None;
        }
        let chart = ccf::Chart {
            meta,
            strings: std::mem::take(&mut self.strings.strings),
            categories: std::mem::take(&mut self.categories),
            floors,
            sections: b.sections,
            chunks: b.chunks,
            rows: b.rows,
            segments: b.segments,
            label_rules: b.label_rules,
            label_exceptions: b.label_exceptions,
            seat_pos: b.seat_pos,
            seat_category: b.seat_category,
            seat_flags: b.seat_flags,
            objects: Vec::new(),
            tables: Vec::new(),
            mesh: ccf::Mesh {
                vertices: b.vertices,
                indices: b.outline_indices,
                draws,
            },
            texts: d.texts,
            icons: d.icons,
            images: d.images,
            full: Some(ccf::Full {
                oids: b.oids,
                ranks,
                priorities: b.priorities,
            }),
        };
        if !f.has_errors() {
            let scene = warnings::Scene {
                doc: self.doc,
                units,
                expansion: e,
                seat_of: &b.seat_of,
                explicit: &b.explicit,
                text_nodes: &d.text_nodes,
                chart: &chart,
            };
            scene.check(f);
        }
        Some(chart)
    }
}

/// Per-row inputs to [`rank`].
#[derive(Clone, Copy)]
struct RowRank {
    floor: usize,
    priority: f64,
}

/// The compiled tables as they are filled, in `idx` order.
#[derive(Default)]
struct Builder<'d> {
    template: &'d str,
    seat_size: u16,
    shape: ccf::SeatShape,
    seat_pos: Vec<Point>,
    seat_category: Vec<u16>,
    seat_flags: Vec<u8>,
    oids: Vec<u32>,
    /// Per `idx`: the seat in the expansion, for regenerating its label.
    seat_of: Vec<u32>,
    /// Per `idx`: hash of the full label, for the uniqueness check.
    label_hash: Vec<(u64, u32)>,
    rows: Vec<ccf::Row>,
    row_rank: Vec<RowRank>,
    /// Per expanded row: its section's label, for regenerating full labels.
    expanded_section: Vec<Option<&'d str>>,
    segments: Vec<ccf::Segment>,
    label_rules: Vec<ccf::SeatLabelRule>,
    rule_index: BTreeMap<RuleKey<'d>, u32>,
    /// The last node's rule: rows of one node share it.
    last_rule: Option<(u32, u32)>,
    label_exceptions: Vec<ccf::LabelException>,
    sections: Vec<ccf::Section>,
    priorities: Vec<f32>,
    chunks: Vec<ccf::RenderChunk>,
    /// The chunk being filled and its scope: floor and section.
    chunk: Option<((usize, u32), ccf::RenderChunk)>,
    floor_seats: Vec<(u32, Option<Bounds>)>,
    /// The current section's seats in its own space, for its outline.
    local: Vec<[f64; 2]>,
    /// `MESH` vertices, and the indices of section outlines in `SECT` order
    /// (then decoration).
    vertices: Vec<ccf::Vertex>,
    outline_indices: Vec<u32>,
    /// Points of explicit outlines so far.
    outline_points: usize,
    /// Per section with an explicit outline: its `SECT` index, node and
    /// world outline, for W104.
    explicit: Vec<(u32, u32, Vec<P>)>,
}

impl<'d> Builder<'d> {
    fn unit(&mut self, c: &mut Compiler<'d>, unit: &Unit<'d>, e: &Expansion<'d>, f: &mut Findings) {
        let section = unit.section.map(|s| {
            let category = s
                .category
                .as_deref()
                .and_then(|k| c.category_index.get(k))
                .map_or(NONE, |&i| u32::from(i));
            let index = self.sections.len() as u32; // ≤ MAX_SECTIONS, else E006
            self.sections.push(ccf::Section {
                label: c.strings.opt(s.label.as_deref()),
                floor: unit.floor as u32, // < MAX_FLOORS
                seat_first: self.seat_pos.len() as u32,
                seat_count: 0,
                row_first: self.rows.len() as u32,
                row_count: 0,
                category,
                entrance: c.strings.opt(s.entrance.as_deref()),
                // An empty range where the outline would start, so that
                // outline ranges tile the start of the index buffer.
                mesh_first: self.outline_indices.len() as u32,
                mesh_count: 0,
                bounds: [0; 4],
                label_anchor: [0, 0],
                label_size: 0,
            });
            self.priorities.push(s.priority as f32);
            (index, s)
        });
        let (seats_before, rows_before) = (self.seat_pos.len(), self.rows.len());
        self.local.clear();
        let mut bounds = None;
        for m in &unit.members {
            let mut problems = Problems::default();
            for r in m.rows.clone() {
                let ctx = RowCtx {
                    unit,
                    section,
                    place: m.place,
                    expanded: r,
                };
                if let Some(row_bounds) = self.row(c, &ctx, e, &mut problems) {
                    grow(&mut bounds, row_bounds);
                }
            }
            // Reported once per node, not once per seat.
            let node = Some(m.node.id());
            if problems.no_category {
                f.push(Code::E002, node, Detail::NoCategory);
            }
            if problems.out_of_range {
                f.push(Code::E005, node, Detail::OutOfRange);
            }
            if problems.too_long {
                f.push(Code::E006, node, Detail::LabelTooLong);
            }
            if let Some((rid, position)) = problems.empty {
                f.push(Code::E003, node, Detail::EmptyLabel { rid, position });
            }
        }
        if let Some((index, _)) = section
            && let Some(s) = self.sections.get_mut(index as usize)
        {
            s.seat_count = (self.seat_pos.len() - seats_before) as u32;
            s.row_count = (self.rows.len() - rows_before) as u32;
            s.bounds = bounds.unwrap_or_default();
        }
        if let Some((index, s)) = section {
            self.outline(unit, index, s, f);
        }
    }

    /// Step 11–12 for one section: its outline (explicit, or computed from its
    /// seats), the outline's triangles, the label anchor and size, and the
    /// bounds grown to cover the outline.
    fn outline(&mut self, unit: &Unit<'d>, index: u32, s: &'d doc::Section, f: &mut Findings) {
        let node = Some(s.id);
        let poly = match &s.outline {
            doc::Outline::Auto => {
                let pitch = unit
                    .members
                    .iter()
                    .filter_map(|m| match m.node {
                        Bookable::Block(b) => Some(b.row_pitch),
                        Bookable::Row(_) => None,
                    })
                    .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.max(p))));
                let padding = pitch.map_or(DEFAULT_PADDING, |p| p / 2.0);
                let Some(poly) = geometry::auto_outline(&self.local, padding) else {
                    f.push(Code::E005, node, Detail::OutOfRange);
                    return;
                };
                poly
            }
            doc::Outline::Points(pts) => {
                self.outline_points = self.outline_points.saturating_add(pts.points.len());
                if self.outline_points > geometry::MAX_OUTLINE_POINTS {
                    let detail = Detail::TooMany {
                        what: "outline points in the chart",
                        limit: geometry::MAX_OUTLINE_POINTS as u32,
                    };
                    f.push(Code::E006, node, detail);
                    return;
                }
                match geometry::explicit_outline(&pts.points) {
                    Ok(poly) => poly,
                    Err(e) => return f.push(outline_code(e), node, outline_detail(e)),
                }
            }
        };
        if poly.is_empty() {
            return;
        }
        // World positions as `MESH` stores them, in `f32`: the outline, its
        // bounds, anchor and label size all use exactly these points. A
        // polygon that rounding makes flat or self-touching is refused.
        let place = unit.place;
        let world = geometry::dedupe(poly.iter().map(|&p| {
            let [x, y] = place.apply(p);
            [f64::from(x as f32), f64::from(y as f32)]
        }));
        let Some(outer) = outer_box(&world) else {
            f.push(Code::E005, node, Detail::OutOfRange);
            return;
        };
        if let Err(e) = geometry::check(&world) {
            f.push(outline_code(e), node, outline_detail(e));
            return;
        }
        let first = self.outline_indices.len();
        let area = geometry::area2(&world).abs();
        let tess = tessellate::fill(
            core::slice::from_ref(&world),
            FillRule::NonZero,
            [area, area],
            OUTLINE_COLOR,
            &mut self.vertices,
            &mut self.outline_indices,
        );
        if let Err(e) = tess {
            f.push(tess_code(e), node, tess_detail(e));
            return;
        }
        let seed = largest_triangle_centroid(&self.vertices, self.outline_indices.get(first..));
        let anchor = quantise_point(geometry::pole(&world, seed)).unwrap_or_default();
        let chars = s.label.as_deref().map_or(0, |l| l.chars().count());
        let at = [f64::from(anchor[0]) / 4.0, f64::from(anchor[1]) / 4.0];
        let size = geometry::label_size(&world, at, chars);
        if matches!(s.outline, doc::Outline::Points(_)) {
            self.explicit.push((index, s.id, world));
        }
        if let Some(rec) = self.sections.get_mut(index as usize) {
            rec.mesh_first = first as u32; // ≤ 3 × MAX_VERTICES
            rec.mesh_count = (self.outline_indices.len() - first) as u32;
            rec.label_anchor = anchor;
            rec.label_size = size;
            rec.bounds = if rec.seat_count > 0 {
                let mut b = rec.bounds;
                grow_box(&mut b, outer);
                b
            } else {
                outer
            };
        }
    }

    /// Emits one expanded row: its seats, segments, label rule, exceptions and
    /// chunk. Returns its bounds, or `None` if it has no seats: a row whose
    /// positions are all removed is not compiled.
    fn row(
        &mut self,
        c: &mut Compiler<'d>,
        ctx: &RowCtx<'_, 'd>,
        e: &Expansion<'d>,
        problems: &mut Problems,
    ) -> Option<Bounds> {
        let row = e.rows.get(ctx.expanded)?;
        let section_label = ctx.section.and_then(|(_, s)| s.label.as_deref());
        if let Some(slot) = self.expanded_section.get_mut(ctx.expanded) {
            *slot = section_label;
        }
        if row.label.as_str().is_empty() {
            problems.empty.get_or_insert((row.rid, None));
        }
        let first = row.first_seat as usize;
        let seats = e.seats.get(first..first + usize::from(row.seat_count))?;
        // Serpentine resolves per row to left-to-right or right-to-left.
        let order = match row.seat_direction {
            SeatDirection::Ltr => ccf::SeatOrder::Ltr,
            SeatDirection::Rtl => ccf::SeatOrder::Rtl,
            SeatDirection::CenterOut => ccf::SeatOrder::CenterOut,
            SeatDirection::Serpentine if row.order.is_multiple_of(2) => ccf::SeatOrder::Ltr,
            SeatDirection::Serpentine => ccf::SeatOrder::Rtl,
        };
        let row_index = self.rows.len() as u32;
        let seat_first = self.seat_pos.len() as u32;
        let limit = row.seat_pitch * row.seat_pitch * 2.25; // (1.5 × pitch)²
        let mut prev: Option<&Seat<'d>> = None;
        let mut bounds = None;
        for (k, seat) in (row.first_seat..).zip(seats) {
            if seat.removed {
                prev = None;
                continue;
            }
            // A segment continues while positions are consecutive and the gap
            // to the previous seat is at most 1.5 × the pitch (node space).
            let joined = prev.is_some_and(|p| {
                let (dx, dy) = (seat.pos[0] - p.pos[0], seat.pos[1] - p.pos[1]);
                p.index + 1 == seat.index && dx * dx + dy * dy <= limit
            });
            prev = Some(seat);
            let idx = self.seat_pos.len() as u32;
            match self.segments.last_mut() {
                Some(g) if joined => g.seat_count += 1,
                _ => self.segments.push(ccf::Segment {
                    row: row_index,
                    seat_first: idx,
                    seat_count: 1,
                    position: seat.index,
                }),
            }

            let local = ctx.place[0].apply(seat.pos);
            if ctx.section.is_some() {
                self.local.push(local);
            }
            let world = ctx.place[1].apply(local);
            let pos = quantise_point(world).unwrap_or_else(|| {
                problems.out_of_range = true;
                [0, 0]
            });
            grow(&mut bounds, point_box(pos));
            let category = seat
                .category
                .or(ctx.section.and_then(|(_, s)| s.category.as_deref()));
            let category = match category {
                // An unknown key was reported by `plan`.
                Some(key) => c.category_index.get(key).copied().unwrap_or(0),
                None => {
                    problems.no_category = true;
                    0
                }
            };
            self.seat_pos.push(pos);
            self.seat_category.push(category);
            self.seat_flags.push(seat.flags);
            self.oids.push(seat.oid);
            self.seat_of.push(k);

            if let Some(text) = seat.label {
                let label = c.strings.get(text);
                self.label_exceptions
                    .push(ccf::LabelException { idx, label });
            }
            let (text, full) = self.labels(row, seat, section_label);
            if text.as_str().is_empty() {
                problems.empty.get_or_insert((row.rid, Some(seat.index)));
            }
            match full {
                Some(full) => self.label_hash.push((fnv1a(full.as_str()), idx)),
                None => problems.too_long = true,
            }
        }
        let count = self.seat_pos.len() as u32 - seat_first;
        if count == 0 {
            return None;
        }
        // Only after a seat: expansion has then written every position's
        // label with this rule, so its parts are within the limits.
        let rule = self.rule(&mut c.strings, row);
        let bounds = bounds.unwrap_or_default();
        let at = |i: u32| self.seat_pos.get(i as usize).copied().unwrap_or_default();
        let last = seat_first + count - 1;
        let label_start = extrapolate(at(seat_first), at((seat_first + 1).min(last)));
        let label_end = extrapolate(at(last), at(last.saturating_sub(1).max(seat_first)));
        let section = ctx.section.map_or(NONE, |(i, _)| i);
        self.rows.push(ccf::Row {
            section,
            label: c.strings.get(row.label.as_str()),
            seat_first,
            seat_count: count as u16, // ≤ positions ≤ MAX_SEATS_PER_ROW
            positions: row.seat_count,
            label_rule: rule,
            ordinal_base: row.seat_base,
            direction: order,
            shape: self.shape,
            seat_size: self.seat_size,
            bounds,
            label_start,
            label_end,
        });
        self.row_rank.push(RowRank {
            floor: ctx.unit.floor,
            priority: ctx.unit.section.map_or(1.0, |s| s.priority),
        });

        // Render chunks: whole rows, greedily, never across a section or floor.
        let scope = (ctx.unit.floor, section);
        let open = self
            .chunk
            .as_mut()
            .filter(|(s, ch)| *s == scope && ch.seat_count + count <= CHUNK_MAX_SEATS);
        match open {
            Some((_, ch)) => {
                ch.seat_count += count;
                grow_box(&mut ch.bounds, bounds);
            }
            None => {
                self.close_chunk();
                let chunk = ccf::RenderChunk {
                    section,
                    seat_first,
                    seat_count: count,
                    bounds,
                };
                self.chunk = Some((scope, chunk));
            }
        }
        if let Some(fl) = self.floor_seats.get_mut(ctx.unit.floor) {
            fl.0 += count;
            grow(&mut fl.1, bounds);
        }
        Some(bounds)
    }

    fn close_chunk(&mut self) {
        if let Some((_, ch)) = self.chunk.take() {
            self.chunks.push(ch);
        }
    }

    /// A seat's label and its full label (`None` past 64 bytes). Expansion
    /// already wrote every position's rule label, so these writes succeed.
    fn labels(
        &self,
        row: &ExpandedRow<'d>,
        seat: &Seat<'d>,
        section: Option<&str>,
    ) -> (LabelBuf, Option<LabelBuf>) {
        let mut text = LabelBuf::new();
        let _ = row.write_seat_label(seat, &mut text);
        let mut full = LabelBuf::new();
        let parts = (
            section.unwrap_or_default(),
            row.label.as_str(),
            text.as_str(),
        );
        let ok = write_full_label(self.template, parts.0, parts.1, parts.2, &mut full);
        (text, ok.ok().map(|()| full))
    }

    /// The `LBLR` index for a row's seat rule, storing each distinct rule once.
    fn rule(&mut self, strings: &mut Interner, row: &ExpandedRow<'d>) -> u32 {
        if let Some((node, i)) = self.last_rule
            && node == row.node
        {
            return i;
        }
        let rule = &row.seat_rule;
        let custom: &'d [String] = if rule.scheme == Scheme::Custom {
            rule.custom
        } else {
            &[]
        };
        let key = (
            rule.scheme as u8,
            rule.overflow as u8,
            rule.start,
            rule.step,
            rule.prefix,
            rule.suffix,
            custom,
        );
        let index = match self.rule_index.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.label_rules.len() as u32; // ≤ rows
                let custom_first = strings.strings.len() as u32;
                for text in custom {
                    if strings.strings.push(text).is_none() {
                        strings.overflow = true;
                    }
                }
                let part = |strings: &mut Interner, s: &str| {
                    if s.is_empty() { NONE } else { strings.get(s) }
                };
                self.label_rules.push(ccf::SeatLabelRule {
                    scheme: rule.scheme,
                    overflow: rule.overflow,
                    start: rule.start,
                    step: rule.step,
                    prefix: part(strings, rule.prefix),
                    suffix: part(strings, rule.suffix),
                    custom_first,
                    custom_count: custom.len() as u32, // strings fit in u32, else overflow
                });
                self.rule_index.insert(key, i);
                i
            }
        };
        self.last_rule = Some((row.node, index));
        index
    }

    /// Step 7: every full label is unique (E001). Labels are compared by hash
    /// after sorting, and equal hashes by the labels themselves, so the check
    /// is O(n log n) and holds one hash per seat.
    fn check_labels(&mut self, e: &Expansion<'d>, f: &mut Findings) {
        self.label_hash.sort_unstable();
        let node_of = |idx: u32| {
            let seat = self
                .seat_of
                .get(idx as usize)
                .and_then(|&s| e.seats.get(s as usize));
            seat.and_then(|s| e.rows.get(s.row as usize))
                .map(|r| r.node)
        };
        for group in self.label_hash.chunk_by(|a, b| a.0 == b.0) {
            if group.len() < 2 {
                continue;
            }
            let mut labels: Vec<(LabelBuf, u32)> = group
                .iter()
                .filter_map(|&(_, idx)| {
                    let s = *self.seat_of.get(idx as usize)?;
                    let seat = e.seats.get(s as usize)?;
                    let row = e.rows.get(seat.row as usize)?;
                    let section = self
                        .expanded_section
                        .get(seat.row as usize)
                        .copied()
                        .flatten();
                    Some((self.labels(row, seat, section).1?, idx))
                })
                .collect();
            labels.sort_unstable_by(|a, b| a.0.as_str().cmp(b.0.as_str()).then(a.1.cmp(&b.1)));
            for same in labels.chunk_by(|a, b| a.0 == b.0) {
                let Some(&(label, first)) = same.first() else {
                    continue;
                };
                let first = node_of(first).unwrap_or_default();
                for &(_, idx) in same.iter().skip(1) {
                    f.push(
                        Code::E001,
                        node_of(idx),
                        Detail::DuplicateLabel { label, first },
                    );
                }
            }
        }
    }
}

fn outline_code(e: OutlineError) -> Code {
    match e {
        OutlineError::TooManyPoints => Code::E006,
        _ => Code::E005,
    }
}

fn outline_detail(e: OutlineError) -> Detail {
    match e {
        OutlineError::NotFinite => Detail::InvalidValue { field: "outline" },
        OutlineError::TooManyPoints => Detail::TooMany {
            what: "outline points",
            limit: geometry::MAX_POINTS as u32,
        },
        OutlineError::ZeroSize => Detail::Geometry {
            problem: "zero size",
        },
        OutlineError::SelfIntersecting => Detail::Geometry {
            problem: "self-intersecting outline",
        },
    }
}

/// The centroid of the largest triangle (the first on a tie): a point
/// strictly inside the polygon they cover.
fn largest_triangle_centroid(vertices: &[ccf::Vertex], indices: Option<&[u32]>) -> P {
    let at = |i: u32| {
        vertices
            .get(i as usize)
            .map_or([0.0; 2], |v| v.pos.map(f64::from))
    };
    let mut best = (0.0, [0.0; 2]);
    for &[i, j, k] in indices.unwrap_or_default().as_chunks::<3>().0 {
        let (a, b, c) = (at(i), at(j), at(k));
        let area = ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs();
        if area > best.0 {
            let centroid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            best = (area, centroid);
        }
    }
    best.1
}

fn tess_code(e: TessError) -> Code {
    match e {
        TessError::TooManyVertices => Code::E006,
        TessError::Failed => Code::E005,
    }
}

fn tess_detail(e: TessError) -> Detail {
    match e {
        TessError::TooManyVertices => Detail::TooMany {
            what: "mesh vertices",
            limit: tessellate::MAX_VERTICES as u32,
        },
        TessError::Failed => Detail::Geometry {
            problem: "cannot be tessellated",
        },
    }
}

/// Problems found per row or seat but reported once per node.
#[derive(Default)]
struct Problems {
    no_category: bool,
    out_of_range: bool,
    too_long: bool,
    /// The first empty row label (no position) or seat label.
    empty: Option<(u32, Option<u16>)>,
}

struct RowCtx<'u, 'd> {
    unit: &'u Unit<'d>,
    section: Option<(u32, &'d doc::Section)>,
    place: [Place; 2],
    /// Index into the expansion's rows.
    expanded: usize,
}

/// `2a − b`: one step beyond `a`, away from its neighbour `b`, saturating.
fn extrapolate(a: Point, b: Point) -> Point {
    let one = |a: i32, b: i32| {
        let v = 2 * i64::from(a) - i64::from(b);
        v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    [one(a[0], b[0]), one(a[1], b[1])]
}

/// FNV-1a, 64-bit: a fast filter for the label uniqueness check. Equal
/// hashes are confirmed by comparing the labels themselves.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn parse_color(s: &str) -> Option<[u8; 4]> {
    let hex = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    let v = u32::from_str_radix(hex, 16)
        .ok()
        .filter(|_| hex.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let [_, r, g, b] = v.to_be_bytes();
    Some([r, g, b, 255])
}

fn node_id(node: &Node) -> u32 {
    match node {
        Node::Section(n) => n.id,
        Node::RowBlock(n) => n.id,
        Node::Row(n) => n.id,
        Node::Table(n) => n.id,
        Node::Booth(n) => n.id,
        Node::Area(n) => n.id,
        Node::Shape(n) => n.id,
        Node::Text(n) => n.id,
        Node::Icon(n) => n.id,
        Node::Image(n) => n.id,
    }
}

fn node_floor(node: &Node) -> Option<u32> {
    match node {
        Node::RowBlock(n) => n.floor,
        Node::Row(n) => n.floor,
        Node::Shape(n) => n.floor,
        Node::Text(n) => n.floor,
        Node::Icon(n) => n.floor,
        Node::Image(n) => n.floor,
        _ => None,
    }
}

fn is_hidden(node: &Node) -> bool {
    match node {
        Node::Section(n) => n.hidden,
        Node::RowBlock(n) => n.hidden,
        Node::Row(n) => n.hidden,
        Node::Table(n) => n.hidden,
        Node::Booth(n) => n.hidden,
        Node::Area(n) => n.hidden,
        Node::Shape(n) => n.hidden,
        Node::Text(n) => n.hidden,
        Node::Icon(n) => n.hidden,
        Node::Image(n) => n.hidden,
    }
}

/// Bookable non-seat objects are not compiled yet and block the compile.
fn unsupported(node: &Node, f: &mut Findings) {
    let kind = match node {
        Node::Table(_) => "table",
        Node::Booth(_) => "booth",
        Node::Area(_) => "area",
        _ => return,
    };
    f.push(
        Code::Unsupported,
        Some(node_id(node)),
        Detail::UnsupportedNode { kind },
    );
}

/// Seat ranks (03 §3.3): the dense rank of (floor order, raw), scaled to
/// 0…65535. Floor order is the dense rank of the floor's level.
fn rank(
    infos: &[FloorInfo<'_>],
    rows: &[ccf::Row],
    row_rank: &[RowRank],
    pos: &[Point],
) -> Vec<u16> {
    let mut levels: Vec<i32> = infos.iter().map(|f| f.level).collect();
    levels.sort_unstable();
    levels.dedup();
    let floor_order = |i: usize| {
        let level = infos.get(i).map_or(0, |f| f.level);
        levels.binary_search(&level).unwrap_or(0) as u32
    };
    let seats = |r: &ccf::Row| {
        let first = r.seat_first as usize;
        pos.get(first..first + usize::from(r.seat_count))
            .unwrap_or_default()
    };
    // Per floor: the sum of seat positions, in quarter units. They are
    // integers below 2³¹ and there are at most 2²⁰ seats, so sums are exact.
    let mut sums = vec![(0.0f64, 0.0f64, 0u32); infos.len()];
    for (r, rr) in rows.iter().zip(row_rank) {
        if let Some((sx, sy, n)) = sums.get_mut(rr.floor) {
            for p in seats(r) {
                (*sx, *sy, *n) = (*sx + f64::from(p[0]), *sy + f64::from(p[1]), *n + 1);
            }
        }
    }
    // Per floor with a focal point: the point and the forward axis, towards
    // the centroid of the floor's seats (+y if they are centred on it).
    let axes: Vec<Option<([f64; 2], [f64; 2])>> = infos
        .iter()
        .zip(&sums)
        .map(|(info, &(sx, sy, n))| {
            let focal = info.focal?;
            let fp = [f64::from(focal[0]), f64::from(focal[1])];
            let mut axis = [0.0, 1.0];
            if n > 0 {
                let (ax, ay) = (sx / f64::from(n) - fp[0], sy / f64::from(n) - fp[1]);
                let len = (ax * ax + ay * ay).sqrt();
                if len > 0.0 {
                    axis = [ax / len, ay / len];
                }
            }
            Some((fp, axis))
        })
        .collect();

    let mut keys: Vec<(u32, f64, u32)> = Vec::with_capacity(pos.len());
    for (ri, (r, rr)) in rows.iter().zip(row_rank).enumerate() {
        let order = floor_order(rr.floor);
        let axis = axes.get(rr.floor).copied().flatten();
        for (idx, p) in (r.seat_first..).zip(seats(r)) {
            let raw = match axis {
                Some((fp, a)) => {
                    let (vx, vy) = (f64::from(p[0]) - fp[0], f64::from(p[1]) - fp[1]);
                    let d = (vx * vx + vy * vy).sqrt();
                    let cos = if d > 0.0 {
                        (vx * a[0] + vy * a[1]) / d
                    } else {
                        1.0
                    };
                    d * (1.0 + 0.6 * (1.0 - cos)) * rr.priority
                }
                // No focal point: rank by row order.
                None => ri as f64,
            };
            keys.push((order, raw, idx));
        }
    }
    keys.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut dense = vec![0u32; pos.len()];
    let mut distinct = 0u32;
    let mut prev: Option<(u32, f64)> = None;
    for &(order, raw, idx) in &keys {
        if let Some((po, pr)) = prev
            && (po != order || pr.total_cmp(&raw).is_ne())
        {
            distinct += 1;
        }
        prev = Some((order, raw));
        if let Some(d) = dense.get_mut(idx as usize) {
            *d = distinct;
        }
    }
    let top = u64::from(distinct.max(1));
    dense
        .into_iter()
        .map(|d| (u64::from(d) * 65_535 / top) as u16)
        .collect()
}

#[cfg(test)]
mod tests;
