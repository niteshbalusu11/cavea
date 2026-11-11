//! The compiled chart in memory: exactly what the CCF chunks carry, as flat
//! arrays (`docs/03-chart-format.md` §4.2). Per-seat data is structure-of-arrays
//! indexed by `idx`; everything else is a table of small `Copy` records whose
//! byte layout is their field order. It is the writer's input and the reader's
//! output, and [`Chart::check`] is the consistency contract between them.

use cavea_core::label::{AlphaOverflow, Scheme, SeatDirection};
use cavea_core::limits::{
    MAX_CATEGORIES, MAX_FLOORS, MAX_LABEL_LEN, MAX_OBJECTS, MAX_SEATS_PER_ROW, MAX_SECTIONS,
};

use crate::codec::{codes, records};
use crate::{Error, ErrorKind, Tag, tags};

/// An absent optional reference (string, section, label rule, table seat range).
pub const NONE: u32 = u32::MAX;
/// Seats in one render chunk (`CHNK`).
pub const CHUNK_MAX_SEATS: u32 = 4_096;

/// Quantised position: x, y in quarter chart units (0.25 cu).
pub type Point = [i32; 2];
/// Quantised box: min x, min y, max x, max y in quarter chart units.
pub type Bounds = [i32; 4];
/// Colour bytes in the order R, G, B, A.
pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeatShape {
    #[default]
    Circle,
    Rounded,
    Chair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RowLabelDisplay {
    None,
    Start,
    End,
    #[default]
    Both,
}

/// A bookable object that is not a seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    /// A table booked whole.
    Table,
    Booth,
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableShape {
    Round,
    Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// A row's seat numbering direction. The compiler resolves
/// [`SeatDirection::Serpentine`] to `Ltr` or `Rtl` per row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatOrder {
    Ltr,
    Rtl,
    CenterOut,
}

impl From<SeatOrder> for SeatDirection {
    fn from(o: SeatOrder) -> Self {
        match o {
            SeatOrder::Ltr => Self::Ltr,
            SeatOrder::Rtl => Self::Rtl,
            SeatOrder::CenterOut => Self::CenterOut,
        }
    }
}

/// Where a mesh draw range sits relative to the seats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Below,
    Above,
}

codes!(SeatShape { 0 => SeatShape::Circle, 1 => SeatShape::Rounded, 2 => SeatShape::Chair });
codes!(RowLabelDisplay {
    0 => RowLabelDisplay::None, 1 => RowLabelDisplay::Start,
    2 => RowLabelDisplay::End, 3 => RowLabelDisplay::Both,
});
codes!(ObjectKind { 0 => ObjectKind::Table, 1 => ObjectKind::Booth, 2 => ObjectKind::Area });
codes!(TableShape { 0 => TableShape::Round, 1 => TableShape::Rect });
codes!(TextAlign { 0 => TextAlign::Left, 1 => TextAlign::Center, 2 => TextAlign::Right });
codes!(Layer { 0 => Layer::Below, 1 => Layer::Above });
codes!(Scheme {
    0 => Scheme::Numeric, 1 => Scheme::Odd, 2 => Scheme::Even, 3 => Scheme::Alpha,
    4 => Scheme::AlphaLower, 5 => Scheme::Roman, 6 => Scheme::Custom,
});
codes!(AlphaOverflow { 0 => AlphaOverflow::Double, 1 => AlphaOverflow::Spreadsheet });
codes!(SeatOrder { 0 => SeatOrder::Ltr, 1 => SeatOrder::Rtl, 2 => SeatOrder::CenterOut });

records! {
    /// `META`: one record.
    Meta {
        name: u32,
        label_template: u32,
        table_label_template: u32,
        /// Compiler name and version, for diagnostics.
        compiler: u32,
        /// Seats; equals the length of every per-seat array.
        seat_count: u32,
        bounds: Bounds,
        /// Default seat size, quarter cu.
        seat_size: u16,
        seat_shape: SeatShape,
        row_label_display: RowLabelDisplay,
        /// Seat rank algorithm (`docs/03-chart-format.md` §3.3).
        rank_algo: u8,
    }

    /// `CATS`.
    Category {
        key: u32,
        label: u32,
        color: Rgba,
        /// Bit 0: accessible. Bits 1–7 reserved, zero.
        flags: u8,
    }

    /// `FLRS`. Floors partition the seats: floor `i` holds seats
    /// `seat_first .. seat_first + seat_count`, in floor order.
    Floor {
        name: u32,
        level: i32,
        seat_first: u32,
        seat_count: u32,
        bounds: Bounds,
        focal: Option<Point>,
    }

    /// `SECT`.
    Section {
        /// String or [`NONE`].
        label: u32,
        floor: u32,
        seat_first: u32,
        seat_count: u32,
        row_first: u32,
        row_count: u32,
        /// Default category, or [`NONE`].
        category: u32,
        /// String or [`NONE`].
        entrance: u32,
        /// Outline triangles: a range of `MESH` indices.
        mesh_first: u32,
        mesh_count: u32,
        bounds: Bounds,
        label_anchor: Point,
        /// Largest label font size that fits, quarter cu.
        label_size: u16,
    }

    /// `CHNK`. Chunks partition the seats, in `idx` order, at most
    /// [`CHUNK_MAX_SEATS`] each.
    RenderChunk {
        /// Section or [`NONE`].
        section: u32,
        seat_first: u32,
        seat_count: u32,
        bounds: Bounds,
    }

    /// `ROWS`. Rows partition the seats in `idx` order; a table booked by seat
    /// is a row of its own. Seats within a row are in physical order.
    Row {
        /// Section or [`NONE`].
        section: u32,
        label: u32,
        seat_first: u32,
        seat_count: u16,
        /// Seat positions including removed ones; seat labels count them.
        positions: u16,
        /// `LBLR` rule for seat labels, or [`NONE`] (labels only from `LBLX`).
        label_rule: u32,
        /// Added to every seat's ordinal: the continuous-numbering base.
        ordinal_base: u32,
        direction: SeatOrder,
        shape: SeatShape,
        /// Quarter cu.
        seat_size: u16,
        bounds: Bounds,
        label_start: Point,
        label_end: Point,
    }

    /// `RSEG`. Each row's seats split into one or more segments, listed in
    /// `idx` order.
    Segment {
        row: u32,
        seat_first: u32,
        seat_count: u16,
        /// Seat position of the segment's first seat in its row.
        position: u16,
    }

    /// `LBLR`. Maps onto [`cavea_core::label::LabelRule`].
    SeatLabelRule {
        scheme: Scheme,
        overflow: AlphaOverflow,
        start: u32,
        step: u32,
        /// String or [`NONE`].
        prefix: u32,
        /// String or [`NONE`].
        suffix: u32,
        /// Custom labels: strings `custom_first .. custom_first + custom_count`.
        custom_first: u32,
        custom_count: u32,
    }

    /// `OBJS`. Object `i` has `idx = seat_count + i`.
    Object {
        kind: ObjectKind,
        label: u32,
        category: u32,
        /// Areas: capacity. Others: 1.
        capacity: u32,
        floor: u32,
        /// Section or [`NONE`].
        section: u32,
        mesh_first: u32,
        mesh_count: u32,
        bounds: Bounds,
        label_anchor: Point,
    }

    /// `TBLS`: every table, however it is booked.
    Table {
        shape: TableShape,
        /// Binary angle: 65,536 per turn, clockwise.
        rotation: u16,
        /// Chairs drawn around the table.
        chairs: u16,
        /// Booked by seat: first of `chairs` seats. Booked whole: [`NONE`].
        seat_first: u32,
        /// Booked whole: index into `OBJS`. Booked by seat: [`NONE`].
        object: u32,
        centre: Point,
        /// Width and height, quarter cu.
        size: Point,
    }

    /// `MESH` vertex: chart units, not quantised.
    Vertex {
        pos: [f32; 2],
        color: Rgba,
    }

    /// `MESH` draw range of decoration: one floor's triangles in one layer.
    /// Draw ranges are sorted by floor, then layer, at most one per pair, and
    /// each starts where the previous one ends.
    Draw {
        floor: u32,
        layer: Layer,
        index_first: u32,
        index_count: u32,
    }

    /// `TEXT`.
    Text {
        text: u32,
        floor: u32,
        pos: Point,
        /// Quarter cu.
        size: u16,
        /// Binary angle.
        angle: u16,
        color: Rgba,
        align: TextAlign,
        weight: u16,
        min_zoom: f32,
    }

    /// `ICON`.
    Icon {
        /// Icon key string: `entrance`, `exit`, …
        key: u32,
        floor: u32,
        pos: Point,
        size: u16,
        angle: u16,
    }

    /// `IMGS`.
    Image {
        /// Asset hash string.
        asset: u32,
        floor: u32,
        bounds: Bounds,
        angle: u16,
        opacity: u8,
    }
}

/// `LBLX`: a seat whose label is not its row rule's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelException {
    pub idx: u32,
    pub label: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<Draw>,
}

/// Data only the full (server) artefact carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Full {
    /// `SOID`: per object `idx`.
    pub oids: Vec<u32>,
    /// `SRNK`: per object `idx`, lower is better.
    pub ranks: Vec<u16>,
    /// `OPRI`: per section.
    pub priorities: Vec<f32>,
}

/// `STRS`: one UTF-8 buffer and the end offset of each string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Strings {
    pub(crate) bytes: String,
    pub(crate) ends: Vec<u32>,
}

impl Strings {
    /// Appends `s` and returns its index, or `None` past 4 GiB.
    pub fn push(&mut self, s: &str) -> Option<u32> {
        let index = u32::try_from(self.ends.len()).ok()?;
        let end = u32::try_from(self.bytes.len().checked_add(s.len())?).ok()?;
        self.bytes.push_str(s);
        self.ends.push(end);
        Some(index)
    }

    pub fn get(&self, index: u32) -> Option<&str> {
        let i = index as usize;
        let start = match i.checked_sub(1) {
            Some(prev) => *self.ends.get(prev)? as usize,
            None => 0,
        };
        self.bytes.get(start..*self.ends.get(i)? as usize)
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }
}

/// A compiled chart. After [`crate::decode`] succeeds, every reference in it is
/// in range (see [`Chart::check`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub meta: Meta,
    pub strings: Strings,
    pub categories: Vec<Category>,
    pub floors: Vec<Floor>,
    pub sections: Vec<Section>,
    pub chunks: Vec<RenderChunk>,
    pub rows: Vec<Row>,
    pub segments: Vec<Segment>,
    pub label_rules: Vec<SeatLabelRule>,
    /// Sorted by `idx`, at most one per seat.
    pub label_exceptions: Vec<LabelException>,
    /// `SPOS`, per seat.
    pub seat_pos: Vec<Point>,
    /// `SCAT`, per seat.
    pub seat_category: Vec<u16>,
    /// `SFLG`, per seat: bit 0 accessible, 1 companion, 2 restricted view,
    /// 3 aisle, 4 removable; 5–7 reserved, zero.
    pub seat_flags: Vec<u8>,
    pub objects: Vec<Object>,
    pub tables: Vec<Table>,
    pub mesh: Mesh,
    pub texts: Vec<Text>,
    pub icons: Vec<Icon>,
    pub images: Vec<Image>,
    /// Present in the full artefact only.
    pub full: Option<Full>,
}

fn within(first: u32, count: u32, len: usize) -> bool {
    u64::from(first) + u64::from(count) <= len as u64
}

fn opt(i: u32, len: usize) -> bool {
    i == NONE || (i as usize) < len
}

/// True if `ranges` are consecutive from 0 and end at `total`.
fn partition(ranges: impl Iterator<Item = (u32, u32)>, total: usize) -> bool {
    let mut next = 0u64;
    for (first, count) in ranges {
        if u64::from(first) != next {
            return false;
        }
        next += u64::from(count);
    }
    next == total as u64
}

impl Chart {
    /// Checks every count against `docs/02-domain-model.md` §14 and every
    /// cross-reference against its table, so that a consumer can index the
    /// decoded arrays without going out of bounds. The writer refuses a chart
    /// that fails it and the reader returns none that does.
    pub fn check(&self) -> Result<(), Error> {
        macro_rules! ensure {
            ($tag:expr, $cond:expr, $kind:ident) => {
                if !$cond {
                    return Err(Error::at($tag, ErrorKind::$kind));
                }
            };
        }
        let str_ok = |i: u32| (i as usize) < self.strings.len();
        let opt_str = |i: u32| opt(i, self.strings.len());
        // Labels stored whole, and label parts, fit `MAX_LABEL_LEN` bytes.
        let short =
            |i: u32| i == NONE || self.strings.get(i).is_none_or(|s| s.len() <= MAX_LABEL_LEN);
        let seats = self.seat_pos.len();
        let cats = self.categories.len();
        let indices = self.mesh.indices.len();
        let objects = seats.saturating_add(self.objects.len());
        let full = self.full.as_ref();
        let limit = |tag: Tag, n: usize, max: u32| -> Result<(), Error> {
            ensure!(tag, n <= max as usize, OverLimit);
            Ok(())
        };
        limit(tags::OBJS, objects, MAX_OBJECTS)?;
        limit(tags::CATS, cats, MAX_CATEGORIES)?;
        limit(tags::FLRS, self.floors.len(), MAX_FLOORS)?;
        limit(tags::SECT, self.sections.len(), MAX_SECTIONS)?;

        let m = &self.meta;
        ensure!(tags::META, m.seat_count as usize == seats, BadCount);
        ensure!(
            tags::META,
            str_ok(m.name)
                && str_ok(m.label_template)
                && str_ok(m.table_label_template)
                && str_ok(m.compiler),
            BadReference
        );
        ensure!(tags::SCAT, self.seat_category.len() == seats, BadCount);
        ensure!(tags::SFLG, self.seat_flags.len() == seats, BadCount);

        for c in &self.categories {
            ensure!(tags::CATS, str_ok(c.key) && str_ok(c.label), BadReference);
        }
        for f in &self.floors {
            ensure!(tags::FLRS, str_ok(f.name), BadReference);
        }
        let floor_ranges = self.floors.iter().map(|f| (f.seat_first, f.seat_count));
        ensure!(tags::FLRS, partition(floor_ranges, seats), BadRange);

        for s in &self.sections {
            ensure!(
                tags::SECT,
                opt_str(s.label)
                    && opt_str(s.entrance)
                    && (s.floor as usize) < self.floors.len()
                    && opt(s.category, cats),
                BadReference
            );
            ensure!(
                tags::SECT,
                within(s.seat_first, s.seat_count, seats)
                    && within(s.row_first, s.row_count, self.rows.len())
                    && within(s.mesh_first, s.mesh_count, indices),
                BadRange
            );
        }

        for c in &self.chunks {
            ensure!(
                tags::CHNK,
                opt(c.section, self.sections.len()),
                BadReference
            );
            ensure!(tags::CHNK, c.seat_count <= CHUNK_MAX_SEATS, OverLimit);
        }
        let chunk_ranges = self.chunks.iter().map(|c| (c.seat_first, c.seat_count));
        ensure!(tags::CHNK, partition(chunk_ranges, seats), BadRange);

        for r in &self.rows {
            ensure!(
                tags::ROWS,
                str_ok(r.label)
                    && opt(r.section, self.sections.len())
                    && opt(r.label_rule, self.label_rules.len()),
                BadReference
            );
            ensure!(
                tags::ROWS,
                u32::from(r.positions) <= MAX_SEATS_PER_ROW,
                OverLimit
            );
            ensure!(tags::ROWS, short(r.label), OverLimit);
            ensure!(tags::ROWS, r.seat_count <= r.positions, BadRange);
            // Seat ordinals run up to `ordinal_base + positions - 1`: they fit
            // a u32, and a custom list covers them (§2.5).
            let end = u64::from(r.ordinal_base) + u64::from(r.positions);
            let custom = self
                .label_rules
                .get(r.label_rule as usize)
                .filter(|l| l.scheme == Scheme::Custom);
            ensure!(
                tags::ROWS,
                end <= u64::from(u32::MAX)
                    && custom.is_none_or(|l| end <= u64::from(l.custom_count)),
                BadRange
            );
        }
        let row_ranges = self
            .rows
            .iter()
            .map(|r| (r.seat_first, u32::from(r.seat_count)));
        ensure!(tags::ROWS, partition(row_ranges, seats), BadRange);

        // Segments split each row, in order, into runs of increasing position.
        let mut segs = self.segments.iter();
        for (index, r) in self.rows.iter().enumerate() {
            let (mut covered, mut position) = (0u32, 0u32);
            while covered < u32::from(r.seat_count) {
                let Some(g) = segs.next() else {
                    return Err(Error::at(tags::RSEG, ErrorKind::BadRange));
                };
                let count = u32::from(g.seat_count);
                ensure!(
                    tags::RSEG,
                    g.row as usize == index
                        && u64::from(g.seat_first) == u64::from(r.seat_first) + u64::from(covered)
                        && count > 0
                        && covered + count <= u32::from(r.seat_count)
                        && u32::from(g.position) >= position
                        && u32::from(g.position) + count <= u32::from(r.positions),
                    BadRange
                );
                covered += count;
                position = u32::from(g.position) + count;
            }
        }
        ensure!(tags::RSEG, segs.next().is_none(), BadRange);

        for l in &self.label_rules {
            ensure!(
                tags::LBLR,
                opt_str(l.prefix)
                    && opt_str(l.suffix)
                    && within(l.custom_first, l.custom_count, self.strings.len()),
                BadReference
            );
            ensure!(tags::LBLR, short(l.prefix) && short(l.suffix), OverLimit);
        }
        let mut next = 0u64;
        for x in &self.label_exceptions {
            ensure!(tags::LBLX, str_ok(x.label), BadReference);
            ensure!(tags::LBLX, short(x.label), OverLimit);
            ensure!(
                tags::LBLX,
                u64::from(x.idx) >= next && (x.idx as usize) < seats,
                BadRange
            );
            next = u64::from(x.idx) + 1;
        }
        ensure!(
            tags::SCAT,
            self.seat_category.iter().all(|&c| usize::from(c) < cats),
            BadReference
        );

        for o in &self.objects {
            ensure!(
                tags::OBJS,
                str_ok(o.label)
                    && (o.category as usize) < cats
                    && (o.floor as usize) < self.floors.len()
                    && opt(o.section, self.sections.len()),
                BadReference
            );
            ensure!(tags::OBJS, short(o.label), OverLimit);
            ensure!(
                tags::OBJS,
                within(o.mesh_first, o.mesh_count, indices),
                BadRange
            );
        }
        for t in &self.tables {
            // Exactly one booking target: the seats or the whole-table object.
            ensure!(
                tags::TBLS,
                (t.seat_first == NONE) != (t.object == NONE),
                BadValue
            );
            ensure!(tags::TBLS, opt(t.object, self.objects.len()), BadReference);
            ensure!(
                tags::TBLS,
                t.seat_first == NONE || within(t.seat_first, u32::from(t.chairs), seats),
                BadRange
            );
        }

        let mesh = &self.mesh;
        ensure!(
            tags::MESH,
            mesh.vertices
                .iter()
                .all(|v| v.pos.iter().all(|p| p.is_finite())),
            BadValue
        );
        ensure!(
            tags::MESH,
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len()),
            BadReference
        );
        let floor_ok = |f: u32| (f as usize) < self.floors.len();
        ensure!(
            tags::MESH,
            mesh.draws.iter().all(|d| floor_ok(d.floor)),
            BadReference
        );
        // Sorted by (floor, layer), at most one per pair, end to end.
        let mut prev: Option<&crate::Draw> = None;
        for d in &mesh.draws {
            let ordered = prev.is_none_or(|p| {
                (p.floor, p.layer as u8) < (d.floor, d.layer as u8)
                    && u64::from(p.index_first) + u64::from(p.index_count)
                        == u64::from(d.index_first)
            });
            ensure!(
                tags::MESH,
                ordered && within(d.index_first, d.index_count, indices),
                BadRange
            );
            prev = Some(d);
        }
        for t in &self.texts {
            ensure!(
                tags::TEXT,
                str_ok(t.text) && floor_ok(t.floor),
                BadReference
            );
            ensure!(tags::TEXT, t.min_zoom.is_finite(), BadValue);
        }
        ensure!(
            tags::ICON,
            self.icons
                .iter()
                .all(|i| str_ok(i.key) && floor_ok(i.floor)),
            BadReference
        );
        ensure!(
            tags::IMGS,
            self.images
                .iter()
                .all(|i| str_ok(i.asset) && floor_ok(i.floor)),
            BadReference
        );

        if let Some(f) = full {
            ensure!(tags::SOID, f.oids.len() == objects, BadCount);
            ensure!(tags::SRNK, f.ranks.len() == objects, BadCount);
            ensure!(
                tags::OPRI,
                f.priorities.len() == self.sections.len(),
                BadCount
            );
            ensure!(
                tags::OPRI,
                f.priorities.iter().all(|p| p.is_finite()),
                BadValue
            );
        }
        Ok(())
    }
}
