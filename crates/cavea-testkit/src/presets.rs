//! The synthetic chart presets of `docs/03-chart-format.md` §7, behind
//! `cavea gen` and the benchmarks of `docs/13-testing-benchmarks.md` §6.2.
//!
//! Every preset is seeded and deterministic: the same preset and seed give the
//! same document, byte for byte, on every platform. The generator uses only
//! `+ − × ÷` and truncation; every angle is a section `rotation` or an `arc`
//! row block, which the compiler expands with its own trigonometry. The seed
//! moves some sections to a cheaper price band and picks which sections hold
//! wheelchair spaces. Each preset compiles with no findings for every seed.
//!
//! `banquet` and `festival` need tables, booths and areas, which the compiler
//! does not compile yet; they are listed in [`WAITING`].

use std::collections::BTreeMap;
use std::f64::consts::PI;

use cavea_compiler::document::{
    BlockRow, Category, ChartDocument, Curve, Floor, Icon, LabelScheme, LabelStart, Layer, Node,
    RowBlock, RowLabels, SeatDirection, SeatFlag, SeatLabels, SeatOverride, Section, Settings,
    Shape, ShapeKind, Style, Text,
};

use crate::synthetic::{ROW_PITCH, SEAT_PITCH, SplitMix64};

/// A preset of 03 §7 that the compiler can compile.
pub struct Preset {
    pub name: &'static str,
    pub seats: u32,
    pub shape: &'static str,
    build: fn(u64) -> ChartDocument,
}

impl Preset {
    /// The preset's chart document for `seed`.
    pub fn generate(&self, seed: u64) -> ChartDocument {
        (self.build)(seed)
    }
}

/// The seed `cavea gen` uses when none is given.
pub const DEFAULT_SEED: u64 = 1;

pub const PRESETS: [Preset; 6] = [
    Preset {
        name: "theatre",
        seats: 2_000,
        shape: "three stalls sections of curved rows, one balcony on a second floor",
        build: theatre,
    },
    Preset {
        name: "arena",
        seats: 20_000,
        shape: "bowl of 40 sections around a rectangular floor",
        build: arena,
    },
    Preset {
        name: "stadium-60k",
        seats: 60_000,
        shape: "two tiers of 60 sections around a pitch",
        build: stadium_60k,
    },
    Preset {
        name: "stadium-100k",
        seats: 100_000,
        shape: "three tiers of 60 sections around a pitch",
        build: stadium_100k,
    },
    Preset {
        name: "stress-250k",
        seats: 250_000,
        shape: "uniform tiers of 60 sections, for limits",
        build: stress_250k,
    },
    Preset {
        name: "stress-500k",
        seats: 500_000,
        shape: "uniform tiers of 60 sections, for limits",
        build: stress_500k,
    },
];

/// Presets of 03 §7 waiting for node types the compiler does not compile yet.
pub const WAITING: [(&str, &str); 2] = [
    (
        "banquet",
        "1,200 seats at 150 round tables of 8; waits for tables",
    ),
    (
        "festival",
        "50 objects, areas and booths only; waits for areas and booths",
    ),
];

pub fn find(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Space left between neighbouring sections, in chart units.
const AISLE: f64 = 110.0;
/// Space between one tier's back row and the next tier's front row.
const CONCOURSE: f64 = 400.0;

/// Price bands from best to cheapest, then the accessible category. Every
/// colour keeps 3:1 against both theme backgrounds and differs from the others
/// (W107).
const CATEGORIES: [(&str, &str, &str); 4] = [
    ("premium", "Premium", "#2563EB"),
    ("standard", "Standard", "#B45309"),
    ("value", "Value", "#047857"),
    ("access", "Wheelchair space", "#C026D3"),
];
const PREMIUM: usize = 0;
const STANDARD: usize = 1;
const VALUE: usize = 2;
const ACCESS: &str = "access";

/// One section: a row block in the section's own space, turned by `rotation`
/// about `centre`.
struct Wedge {
    label: String,
    centre: [f64; 2],
    rotation: f64,
    /// Block origin in the section's space; `front` is the front row's
    /// distance from `centre`.
    x: f64,
    front: f64,
    kind: Kind,
    /// Price band, an index into [`CATEGORIES`].
    band: usize,
    row_labels: RowLabels,
    seat_labels: SeatLabels,
}

enum Kind {
    /// Straight rows in a strip this wide, aisle included.
    Straight { width: f64 },
    /// Concentric arcs about `centre` over this angle in radians, aisle included.
    Arc { span: f64 },
}

impl Wedge {
    /// Seats that fit in row `row`. A row that would hold a single seat (W103)
    /// holds none.
    fn capacity(&self, row: u32) -> u32 {
        let length = match self.kind {
            Kind::Straight { width } => width,
            Kind::Arc { span } => (self.front + f64::from(row) * ROW_PITCH) * span,
        } - AISLE;
        if length < SEAT_PITCH {
            return 0;
        }
        ((length / SEAT_PITCH) as u32 + 1).min(4_095)
    }
}

/// Fills `wedges` front to back with up to `left` seats, at most `rows` rows
/// deep (`None`: until the seats run out). Every row but the last is full; the
/// last is shared out in proportion to what each section could hold. Returns
/// the seat count of every row of every wedge.
fn fill(wedges: &[Wedge], rows: Option<u32>, left: &mut u32) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); wedges.len()];
    let mut row = 0;
    while *left > 0 && rows.is_none_or(|rows| row < rows) {
        let caps: Vec<u32> = wedges.iter().map(|w| w.capacity(row)).collect();
        let total: u32 = caps.iter().sum();
        if total == 0 {
            break;
        }
        let take = if *left >= total {
            caps
        } else {
            share(&caps, *left)
        };
        for (seats, n) in out.iter_mut().zip(take) {
            if n > 0 {
                seats.push(n);
            }
        }
        *left -= total.min(*left);
        row += 1;
    }
    out
}

/// Shares `m` seats, fewer than the sum of `caps`, in proportion to `caps`,
/// with no row of a single seat.
fn share(caps: &[u32], m: u32) -> Vec<u32> {
    let total: u64 = caps.iter().map(|&c| u64::from(c)).sum();
    let mut out: Vec<u32> = caps
        .iter()
        .map(|&c| (u64::from(m) * u64::from(c) / total) as u32)
        .collect();
    // Each non-empty row has room for one more and the remainder is below
    // their number.
    let mut rest = m - out.iter().sum::<u32>();
    for (n, &cap) in out.iter_mut().zip(caps) {
        if rest > 0 && *n < cap {
            *n += 1;
            rest -= 1;
        }
    }
    // A lone seat joins another row with room.
    for i in 0..out.len() {
        if out.get(i) != Some(&1) {
            continue;
        }
        let room = |j: usize, n: u32| j != i && n >= 1 && caps.get(j).is_some_and(|&c| n < c);
        if let Some(j) = out.iter().enumerate().position(|(j, &n)| room(j, n)) {
            if let Some(n) = out.get_mut(j) {
                *n += 1;
            }
            if let Some(n) = out.get_mut(i) {
                *n = 0;
            }
        }
    }
    out
}

/// Builds a document, allocating ids from one counter.
struct Builder {
    next: u32,
    nodes: Vec<Node>,
    rng: SplitMix64,
}

impl Builder {
    fn new(seed: u64) -> Self {
        Self {
            next: 1,
            nodes: Vec::new(),
            rng: SplitMix64(seed),
        }
    }

    fn id(&mut self) -> u32 {
        self.next += 1;
        self.next - 1
    }

    /// Adds `wedge` as a section of `rows` (seat counts, front to back). With
    /// `access`, the two seats at each end of the front row are wheelchair
    /// spaces and their inner neighbours companion seats.
    fn section(&mut self, wedge: &Wedge, rows: &[u32], floor: Option<u32>, access: bool) {
        if rows.is_empty() {
            return;
        }
        let id = self.id();
        let block_id = self.id();
        let mut block_rows = Vec::with_capacity(rows.len());
        let mut overrides = BTreeMap::new();
        for (i, &n) in rows.iter().enumerate() {
            let rid = self.id();
            let first = self.next;
            self.next += n;
            if i == 0 && access && n >= 8 {
                for k in [0, 1, n - 2, n - 1] {
                    overrides.insert(first + k, flagged(SeatFlag::Accessible, Some(ACCESS)));
                }
                for k in [2, n - 3] {
                    overrides.insert(first + k, flagged(SeatFlag::Companion, None));
                }
            }
            block_rows.push(BlockRow {
                rid,
                seats: n,
                offset: 0.0,
                oids: vec![[first, n]],
                label: None,
                removed: Vec::new(),
            });
        }
        let block = RowBlock {
            id: block_id,
            name: None,
            floor: None,
            origin: [wedge.x, wedge.front],
            rotation: 0.0,
            locked: false,
            hidden: false,
            seat_pitch: SEAT_PITCH,
            row_pitch: ROW_PITCH,
            curve: match wedge.kind {
                Kind::Straight { .. } => Curve::None {},
                Kind::Arc { .. } => Curve::Arc {
                    radius: wedge.front,
                },
            },
            align: Default::default(),
            stagger: 0.0,
            skew: 0.0,
            category: None,
            row_labels: wedge.row_labels.clone(),
            seat_labels: wedge.seat_labels.clone(),
            rows: block_rows,
            seat_overrides: overrides,
        };
        let category = CATEGORIES.get(wedge.band).map(|c| c.0.to_owned());
        self.nodes.push(Node::Section(Section {
            id,
            name: None,
            floor,
            origin: wedge.centre,
            rotation: wedge.rotation,
            locked: false,
            hidden: false,
            label: Some(wedge.label.clone()),
            outline: Default::default(),
            category,
            priority: 1.0,
            entrance: None,
            children: vec![Node::RowBlock(block)],
        }));
    }

    /// A seeded nudge: about one in six standard sections becomes value.
    fn band(&mut self, band: usize) -> usize {
        if band == STANDARD && self.rng.next().is_multiple_of(6) {
            VALUE
        } else {
            band
        }
    }

    fn shape(
        &mut self,
        floor: Option<u32>,
        origin: [f64; 2],
        kind: ShapeKind,
        style: Style,
    ) -> Shape {
        Shape {
            id: self.id(),
            name: None,
            floor,
            origin,
            rotation: 0.0,
            locked: false,
            hidden: false,
            kind,
            size: None,
            corner_radius: None,
            points: None,
            d: None,
            style,
            layer: Layer::Below,
        }
    }

    fn rect(
        &mut self,
        floor: Option<u32>,
        centre: [f64; 2],
        size: [f64; 2],
        radius: f64,
        style: Style,
    ) {
        let mut shape = self.shape(floor, centre, ShapeKind::Rect, style);
        shape.size = Some(size);
        shape.corner_radius = (radius > 0.0).then_some(radius);
        self.nodes.push(Node::Shape(shape));
    }

    fn ellipse(&mut self, floor: Option<u32>, centre: [f64; 2], size: [f64; 2], style: Style) {
        let mut shape = self.shape(floor, centre, ShapeKind::Ellipse, style);
        shape.size = Some(size);
        self.nodes.push(Node::Shape(shape));
    }

    fn line(&mut self, floor: Option<u32>, points: Vec<[f64; 2]>, style: Style) {
        let mut shape = self.shape(floor, [0.0, 0.0], ShapeKind::Line, style);
        shape.points = Some(points);
        self.nodes.push(Node::Shape(shape));
    }

    fn text(&mut self, floor: Option<u32>, origin: [f64; 2], text: &str, size: f64, color: &str) {
        let id = self.id();
        self.nodes.push(Node::Text(Text {
            id,
            name: None,
            floor,
            origin,
            rotation: 0.0,
            locked: false,
            hidden: false,
            text: text.to_owned(),
            size,
            weight: 600,
            color: color.to_owned(),
            align: Default::default(),
            min_zoom: 0.0,
        }));
    }

    fn icon(&mut self, floor: Option<u32>, origin: [f64; 2], icon: &str, size: f64) {
        let id = self.id();
        self.nodes.push(Node::Icon(Icon {
            id,
            name: None,
            floor,
            origin,
            rotation: 0.0,
            locked: false,
            hidden: false,
            icon: icon.to_owned(),
            size,
        }));
    }

    fn finish(self, name: String, floors: Vec<Floor>) -> ChartDocument {
        ChartDocument {
            schema: cavea_compiler::SCHEMA,
            name,
            settings: Settings::default(),
            next_oid: self.next,
            categories: CATEGORIES
                .iter()
                .map(|&(key, label, color)| Category {
                    key: key.to_owned(),
                    label: label.to_owned(),
                    color: color.to_owned(),
                    accessible: key == ACCESS,
                })
                .collect(),
            floors,
            nodes: self.nodes,
        }
    }
}

fn flagged(flag: SeatFlag, category: Option<&str>) -> SeatOverride {
    SeatOverride {
        flags: vec![flag],
        category: category.map(str::to_owned),
        label: None,
        nudge: None,
    }
}

fn fill_style(fill: &str) -> Style {
    Style {
        fill: Some(fill.to_owned()),
        ..Style::default()
    }
}

fn stroke_style(stroke: &str, width: f64) -> Style {
    Style {
        stroke: Some(stroke.to_owned()),
        stroke_width: width,
        ..Style::default()
    }
}

fn floor(id: u32, name: &str, level: i32, focal: [f64; 2]) -> Floor {
    Floor {
        id,
        name: name.to_owned(),
        level,
        focal_point: Some(focal),
    }
}

/// Rows A, B, C … without I and O, from the front.
fn alpha_rows() -> RowLabels {
    RowLabels {
        skip: vec!["I".into(), "O".into()],
        ..RowLabels::default()
    }
}

fn numeric_rows() -> RowLabels {
    RowLabels {
        scheme: LabelScheme::Numeric,
        ..RowLabels::default()
    }
}

fn seats(scheme: LabelScheme, start: Option<u32>, direction: SeatDirection) -> SeatLabels {
    SeatLabels {
        scheme,
        start: start.map(LabelStart::Number),
        direction,
        ..SeatLabels::default()
    }
}

/// A traditional theatre: the stage at the top, three stalls sections in a
/// fan of curved rows (house left numbered in even seats, centre from 101,
/// house right in odd seats), and one balcony on a second floor that
/// overhangs the back of the stalls.
fn theatre(seed: u64) -> ChartDocument {
    const STALLS: u32 = 1_400;
    const BALCONY: u32 = 600;
    // Rows are arcs about a point behind the stage's front edge (y = 0).
    let centre = [0.0, -1_200.0];
    let front = 1_700.0;
    let span = PI / 6.0;
    let mut b = Builder::new(seed);

    let stalls = [
        (
            "Stalls Left",
            30.0,
            LabelScheme::Even,
            None,
            SeatDirection::Rtl,
        ),
        (
            "Stalls Centre",
            0.0,
            LabelScheme::Numeric,
            Some(101),
            SeatDirection::Ltr,
        ),
        (
            "Stalls Right",
            -30.0,
            LabelScheme::Odd,
            None,
            SeatDirection::Ltr,
        ),
    ];
    let wedges: Vec<Wedge> = stalls
        .into_iter()
        .map(|(label, rotation, scheme, start, direction)| Wedge {
            label: label.to_owned(),
            centre,
            rotation,
            x: 0.0,
            front,
            kind: Kind::Arc { span },
            band: 0,
            row_labels: alpha_rows(),
            seat_labels: seats(scheme, start, direction),
        })
        .collect();
    let mut left = STALLS;
    let rows = fill(&wedges, None, &mut left);
    // The centre is premium, the sides standard, and the seed may make one
    // side value.
    let cheap = b.rng.next() % 3;
    for (i, (mut wedge, rows)) in wedges.into_iter().zip(rows).enumerate() {
        wedge.band = match i {
            1 => PREMIUM,
            _ if i as u64 == cheap => VALUE,
            _ => STANDARD,
        };
        b.section(&wedge, &rows, Some(1), i != 1);
    }

    let balcony = Wedge {
        label: "Balcony".to_owned(),
        centre,
        rotation: 0.0,
        x: 0.0,
        front: front + 12.0 * ROW_PITCH,
        kind: Kind::Arc {
            span: PI * 5.0 / 9.0,
        },
        band: VALUE,
        row_labels: alpha_rows(),
        seat_labels: seats(LabelScheme::Numeric, None, SeatDirection::Ltr),
    };
    let mut left = BALCONY;
    let rows = fill(std::slice::from_ref(&balcony), None, &mut left);
    if let Some(rows) = rows.first() {
        b.section(&balcony, rows, Some(2), true);
    }

    for floor in [1, 2] {
        b.rect(
            Some(floor),
            [0.0, -450.0],
            [2_400.0, 900.0],
            0.0,
            fill_style("#44403C"),
        );
        b.text(Some(floor), [0.0, -450.0], "STAGE", 200.0, "#FAFAF9");
        let edge = vec![[-1_200.0, 0.0], [1_200.0, 0.0]];
        b.line(Some(floor), edge, stroke_style("#A8A29E", 20.0));
    }
    b.icon(Some(1), [-2_700.0, 2_300.0], "exit", 140.0);
    b.icon(Some(1), [2_700.0, 2_300.0], "exit", 140.0);
    b.icon(Some(2), [0.0, 2_900.0], "bar", 140.0);

    let floors = vec![
        floor(1, "Stalls", 0, [0.0, 0.0]),
        floor(2, "Balcony", 1, [0.0, 0.0]),
    ];
    b.finish(format!("Theatre, 2,000 seats (seed {seed})"), floors)
}

/// One tier of a bowl.
struct Tier {
    /// Rows deep; `None` for the last tier, which takes the remaining seats.
    rows: Option<u32>,
    /// Sections on each long side, each short side and each corner.
    long: u32,
    short: u32,
    corner: u32,
    /// Price band of the long sides' central sections; the other side
    /// sections are one band cheaper and the corners two, at most [`VALUE`].
    band: usize,
}

/// A bowl around a rectangular field: straight rows along the sides and arcs
/// at the corners, the front row a rounded rectangle of half-size
/// `half` and corner radius `corner`, tiers one behind the other.
struct Bowl {
    seats: u32,
    half: [f64; 2],
    corner: f64,
    tiers: &'static [Tier],
}

impl Bowl {
    /// Adds the sections, numbered by tier (101, 102 … 201 …) clockwise from
    /// the right-hand end of the bottom side.
    fn build(&self, b: &mut Builder) {
        let [a, h] = [self.half[0] - self.corner, self.half[1] - self.corner];
        let mut left = self.seats;
        let mut front = self.corner;
        for (t, tier) in self.tiers.iter().enumerate() {
            let mut wedges = Vec::new();
            // Clockwise on screen: bottom, bottom-left corner, left, and so on.
            // A section's +y points away from the field; its −x is clockwise.
            for quarter in 0..4u32 {
                let rotation = f64::from(quarter) * 90.0;
                let (n, half, distance) = if quarter % 2 == 0 {
                    (tier.long, a, h)
                } else {
                    (tier.short, h, a)
                };
                let width = 2.0 * half / f64::from(n);
                for k in 0..n {
                    let central = 4 * k >= n && 4 * k < 3 * n && quarter % 2 == 0;
                    wedges.push(Wedge {
                        label: String::new(),
                        centre: [0.0, 0.0],
                        rotation,
                        x: half - (f64::from(k) + 0.5) * width,
                        front: distance + front,
                        kind: Kind::Straight { width },
                        band: if central { tier.band } else { tier.band + 1 },
                        row_labels: if t == 0 { alpha_rows() } else { numeric_rows() },
                        seat_labels: seats(LabelScheme::Numeric, None, SeatDirection::Ltr),
                    });
                }
                let centre = [[-a, h], [-a, -h], [a, -h], [a, h]];
                let span = 90.0 / f64::from(tier.corner);
                for k in 0..tier.corner {
                    wedges.push(Wedge {
                        label: String::new(),
                        centre: centre.get(quarter as usize).copied().unwrap_or_default(),
                        rotation: rotation + (f64::from(k) + 0.5) * span,
                        x: 0.0,
                        front,
                        kind: Kind::Arc {
                            span: PI / 2.0 / f64::from(tier.corner),
                        },
                        band: tier.band + 2,
                        row_labels: if t == 0 { alpha_rows() } else { numeric_rows() },
                        seat_labels: seats(LabelScheme::Numeric, None, SeatDirection::Ltr),
                    });
                }
            }
            let rows = fill(&wedges, tier.rows, &mut left);
            let depth = rows.iter().map(Vec::len).max().unwrap_or(0) as u32;
            for (k, (mut wedge, rows)) in wedges.into_iter().zip(rows).enumerate() {
                wedge.label = format!("{}", (t + 1) * 100 + k + 1);
                wedge.band = wedge.band.min(VALUE);
                // Odd sections only, so that every band stays in use.
                if k % 2 == 1 {
                    wedge.band = b.band(wedge.band);
                }
                // Wheelchair spaces at the front of the lower tier: in the
                // first section and a seeded fifth of the others.
                let access = t == 0 && (k == 0 || b.rng.next().is_multiple_of(5));
                b.section(&wedge, &rows, None, access);
            }
            front += f64::from(depth) * ROW_PITCH + CONCOURSE;
        }
    }
}

fn arena(seed: u64) -> ChartDocument {
    const TIERS: &[Tier] = &[Tier {
        rows: None,
        long: 7,
        short: 3,
        corner: 5,
        band: PREMIUM,
    }];
    let mut b = Builder::new(seed);
    Bowl {
        seats: 20_000,
        half: [3_600.0, 2_100.0],
        corner: 1_200.0,
        tiers: TIERS,
    }
    .build(&mut b);
    // The floor: a rink-shaped board with a centre line and circle.
    b.rect(
        None,
        [0.0, 0.0],
        [6_000.0, 3_000.0],
        850.0,
        fill_style("#E7E5E4"),
    );
    let line = stroke_style("#B91C1C", 30.0);
    b.line(None, vec![[0.0, -1_500.0], [0.0, 1_500.0]], line.clone());
    b.ellipse(None, [0.0, 0.0], [900.0, 900.0], line);
    b.text(None, [0.0, -900.0], "FLOOR", 300.0, "#44403C");
    b.icon(None, [-3_300.0, 0.0], "entrance", 200.0);
    b.icon(None, [3_300.0, 0.0], "entrance", 200.0);
    let floors = vec![floor(1, "Arena", 0, [0.0, 0.0])];
    b.finish(format!("Arena, 20,000 seats (seed {seed})"), floors)
}

/// A football stadium of `seats` in `tiers` around a 105 × 68 m pitch, its
/// label `text` chart units high so that it stays readable at fit zoom (W108).
fn stadium(name: &str, seats: u32, tiers: &'static [Tier], text: f64, seed: u64) -> ChartDocument {
    let mut b = Builder::new(seed);
    Bowl {
        seats,
        half: [6_150.0, 4_300.0],
        corner: 2_000.0,
        tiers,
    }
    .build(&mut b);
    b.rect(
        None,
        [0.0, 0.0],
        [10_500.0, 6_800.0],
        0.0,
        fill_style("#15803D"),
    );
    let line = stroke_style("#F0FDF4", 15.0);
    b.line(None, vec![[0.0, -3_400.0], [0.0, 3_400.0]], line.clone());
    b.ellipse(None, [0.0, 0.0], [1_830.0, 1_830.0], line.clone());
    for x in [-1.0, 1.0] {
        let mut boxed = line.clone();
        boxed.fill = None;
        b.rect(
            None,
            [x * (5_250.0 - 825.0), 0.0],
            [1_650.0, 4_030.0],
            0.0,
            boxed,
        );
    }
    b.text(None, [0.0, -2_200.0], "PITCH", text, "#F0FDF4");
    for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        b.icon(None, [x * 5_800.0, y * 3_900.0], "entrance", 300.0);
    }
    let floors = vec![floor(1, "Stadium", 0, [0.0, 0.0])];
    b.finish(format!("{name} (seed {seed})"), floors)
}

/// The tiers of the stadiums: 60 sections each, the best seats on the long
/// sides of the lower tiers.
const fn tier(rows: Option<u32>, band: usize) -> Tier {
    Tier {
        rows,
        long: 12,
        short: 6,
        corner: 6,
        band,
    }
}

fn stadium_60k(seed: u64) -> ChartDocument {
    const TIERS: &[Tier] = &[tier(Some(30), PREMIUM), tier(None, 1)];
    stadium("Stadium, 60,000 seats", 60_000, TIERS, 600.0, seed)
}

fn stadium_100k(seed: u64) -> ChartDocument {
    const TIERS: &[Tier] = &[
        tier(Some(30), PREMIUM),
        tier(Some(20), PREMIUM),
        tier(None, 1),
    ];
    stadium("Stadium, 100,000 seats", 100_000, TIERS, 600.0, seed)
}

/// Uniform tiers of 30 rows; the last takes what is left.
const STRESS: &[Tier] = &[
    tier(Some(30), PREMIUM),
    tier(Some(30), 1),
    tier(Some(30), 1),
    tier(Some(30), 1),
    tier(Some(30), 1),
    tier(Some(30), 1),
    tier(None, 1),
];

fn stress_250k(seed: u64) -> ChartDocument {
    stadium("Stress, 250,000 seats", 250_000, STRESS, 1_000.0, seed)
}

fn stress_500k(seed: u64) -> ChartDocument {
    stadium("Stress, 500,000 seats", 500_000, STRESS, 1_000.0, seed)
}

#[cfg(test)]
mod tests;
