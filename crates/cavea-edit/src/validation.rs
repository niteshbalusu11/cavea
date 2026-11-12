//! Continuous validation (`docs/07-designer.md` §10, §14): the findings of
//! 03 §3.2 kept up to date as the document changes, by the compiler's own
//! rules, since validation is compiling (03 §3.2).
//!
//! Two kinds of work, each an indivisible piece the shell runs in idle time
//! through [`Validator::step`]:
//!
//! - **Units.** A top-level node with its subtree is compiled alone, in a
//!   document holding only it and the chart-level data. Its errors that
//!   depend on nothing outside it ([`local`]) are exactly what the whole
//!   chart's compile reports for its nodes, so a unit is recompiled only
//!   when it, or the chart's settings, categories or floors, change. The
//!   chart-level errors (categories, floors, settings) come from a compile
//!   with no nodes.
//! - **The full pass.** One compile of the whole chart, for what no unit
//!   can know: duplicate labels and ids across nodes (E001, E009), the
//!   chart-wide counts (E006: objects, sections, outline and shape points,
//!   mesh size, decoration counts), and every warning, which the compiler
//!   checks only on a chart without errors. Until it has run on the
//!   document as it is, the list is marked incomplete and carries the last
//!   pass's findings of those kinds as stale.
//!
//! Each piece is atomic, so a pass interrupted by an edit is abandoned
//! between pieces: the next step starts from what changed.

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::{Category, Floor, Node, Settings};
use cavea_compiler::{ChartDocument, CompileError, Detail, ExpandErrorKind, Finding, Severity};
use serde::Serialize;

use crate::command::expand_runs;
use crate::document::Document;
use crate::views::NodeKind;
use crate::index::{Rect, SpatialIndex};

/// Whether the compiler can report `f` from one top-level node alone: not
/// a warning, not a duplicate across nodes, not a chart-wide count.
pub(crate) fn local(f: &Finding) -> bool {
    if f.severity() == Severity::Warning {
        return false;
    }
    match &f.detail {
        Detail::DuplicateLabel { .. } | Detail::DuplicateId { .. } => false,
        Detail::Expand(ExpandErrorKind::TooManyObjects) => false,
        // Per outline or per shape; every other count is the chart's.
        Detail::TooMany { what, .. } => {
            f.node.is_none() || matches!(*what, "outline points" | "shape points")
        }
        _ => true,
    }
}

/// A chart-wide count exceeded (E006): the compiler then leaves out the
/// nodes past it, so their own errors may be missing from its list.
pub(crate) fn limit(f: &Finding) -> bool {
    match &f.detail {
        Detail::Expand(ExpandErrorKind::TooManyObjects) => true,
        Detail::TooMany { .. } => !local(f),
        _ => false,
    }
}

/// The findings of compiling `doc`: the errors of one that does not
/// compile, else its warnings. An encoding failure, which only a chart
/// without errors can reach, is returned as its message.
fn compile(doc: &ChartDocument) -> (Vec<Finding>, Option<String>) {
    match cavea_compiler::compile(doc) {
        Ok(c) => (c.findings, None),
        Err(CompileError::Invalid(f)) => (f, None),
        Err(e @ CompileError::Encode(_)) => (Vec::new(), Some(e.to_string())),
    }
}

/// Chart-level data the units were compiled against.
#[derive(Debug, Clone, PartialEq)]
struct Basis {
    settings: Settings,
    categories: Vec<Category>,
    floors: Vec<Floor>,
}

#[derive(Debug)]
struct Full {
    revision: u64,
    findings: Vec<Finding>,
    encode: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Validator {
    basis: Option<Basis>,
    /// Chart-level errors, for `basis`.
    chart: Vec<Finding>,
    /// Per top-level node: its local errors.
    units: BTreeMap<u32, Vec<Finding>>,
    /// Top-level nodes to compile again.
    dirty: BTreeSet<u32>,
    /// The document revision last synced.
    synced: Option<u64>,
    full: Option<Full>,
    /// Pieces of work done: the view cannot change without it or the
    /// document's revision.
    pub(crate) pieces: u64,
}

/// What a step left to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Nothing left: the list is the compiler's for the document as it is.
    pub done: bool,
    /// The cost of the next piece, in the budget's units (seat positions,
    /// plus one per piece); 0 when done.
    pub next: u64,
}

impl Validator {
    /// Notes what changed since the last sync; does no validation.
    pub(crate) fn sync(&mut self, doc: &Document) {
        let Some(since) = self.synced else {
            self.dirty = doc.children(None).iter().copied().collect();
            self.synced = Some(doc.revision());
            return;
        };
        if since == doc.revision() {
            return;
        }
        let changes = doc.changes_since(since);
        for id in &changes.removed {
            self.units.remove(id);
            self.dirty.remove(id);
        }
        for &id in &changes.nodes {
            // A node moved into a section is no longer a unit of its own.
            if doc.parent(id).is_some() {
                self.units.remove(&id);
                self.dirty.remove(&id);
            }
            let mut top = id;
            while let Some(p) = doc.parent(top) {
                top = p;
            }
            self.dirty.insert(top);
        }
        if changes.chart && self.basis.as_ref() != Some(&basis(doc)) {
            self.basis = None;
            self.dirty.extend(doc.children(None).iter().copied());
        }
        self.synced = Some(doc.revision());
    }

    fn complete(&self, doc: &Document) -> bool {
        self.full
            .as_ref()
            .is_some_and(|f| f.revision == doc.revision())
    }

    fn unit_cost(doc: &Document, index: &SpatialIndex, id: u32) -> u64 {
        let mut n = 1 + index.seats(id).len() as u64;
        for &c in doc.children(Some(id)) {
            n += index.seats(c).len() as u64;
        }
        n
    }

    /// Does pieces of work until their cost reaches `budget`, at least one.
    pub(crate) fn step(&mut self, doc: &Document, index: &SpatialIndex, budget: u64) -> Progress {
        self.sync(doc);
        let mut spent = 0u64;
        loop {
            let cost = if self.basis.is_none() {
                self.basis = Some(basis(doc));
                self.chart = compile(&chart_only(doc, Vec::new()))
                    .0
                    .into_iter()
                    .filter(|f| f.node.is_none() && local(f))
                    .collect();
                1
            } else if let Some(id) = self.dirty.pop_first() {
                let cost = Self::unit_cost(doc, index, id);
                let unit = doc.tree(id).into_iter().collect();
                let found: Vec<Finding> = compile(&chart_only(doc, unit))
                    .0
                    .into_iter()
                    .filter(|f| f.node.is_some() && local(f))
                    .collect();
                if found.is_empty() {
                    self.units.remove(&id);
                } else {
                    self.units.insert(id, found);
                }
                cost
            } else if !self.complete(doc) {
                let (findings, encode) = compile(&doc.snapshot());
                self.full = Some(Full {
                    revision: doc.revision(),
                    findings,
                    encode,
                });
                1 + doc
                    .ids()
                    .map(|id| index.seats(id).len() as u64)
                    .sum::<u64>()
            } else {
                break;
            };
            self.pieces += 1;
            spent = spent.saturating_add(cost);
            if spent >= budget {
                break;
            }
        }
        self.progress(doc, index)
    }

    pub(crate) fn progress(&self, doc: &Document, index: &SpatialIndex) -> Progress {
        let next = if self.basis.is_none() {
            1
        } else if let Some(&id) = self.dirty.first() {
            Self::unit_cost(doc, index, id)
        } else if !self.complete(doc) {
            1 + doc
                .ids()
                .map(|id| index.seats(id).len() as u64)
                .sum::<u64>()
        } else {
            0
        };
        Progress {
            done: next == 0,
            next,
        }
    }

    pub(crate) fn view(&self, doc: &Document) -> ValidationView {
        let complete = self.complete(doc) && self.dirty.is_empty();
        let mut items: Vec<(&Finding, bool)> = Vec::new();
        let mut encode = None;
        match &self.full {
            Some(full) if complete => {
                items.extend(full.findings.iter().map(|f| (f, false)));
                encode = full.encode.clone();
            }
            _ => {
                // Chart-level errors are stale while the chart has changed.
                let changed = self.basis.is_none();
                items.extend(self.chart.iter().map(|f| (f, changed)));
                // Past a chart-wide limit the compiler stops checking the
                // nodes beyond it, which a unit alone still reports: the
                // units' findings are then only a guide until the pass.
                let limited = self.full.iter().flat_map(|f| &f.findings).any(limit);
                let units = self.units.iter().flat_map(|(id, f)| {
                    let stale = limited || self.dirty.contains(id);
                    f.iter().map(move |f| (f, stale))
                });
                items.extend(units);
                // The last pass's findings no unit can give, while they may
                // be out of date.
                let exists = |n: Option<u32>| n.is_none_or(|n| doc.node(n).is_some());
                let old = self.full.iter().flat_map(|f| &f.findings);
                items.extend(
                    old.filter(|f| !local(f) && exists(f.node))
                        .map(|f| (f, true)),
                );
            }
        }
        let findings: Vec<FindingView> = items
            .into_iter()
            .map(|(f, stale)| finding_view(doc, f, stale))
            .collect::<Vec<_>>();
        let mut findings = findings;
        findings.sort_by(|a, b| {
            (a.severity != "error")
                .cmp(&(b.severity != "error"))
                .then(a.code.cmp(b.code))
                .then(a.node.cmp(&b.node))
        });
        let errors = findings.iter().filter(|f| f.severity == "error").count() as u32;
        let warnings = findings.len() as u32 - errors;
        ValidationView {
            publishable: complete && errors == 0 && encode.is_none(),
            findings,
            errors,
            warnings,
            complete,
            encode,
        }
    }
}

#[cfg(test)]
impl Validator {
    /// Every unit, and the chart-level errors, are compiled for the
    /// document as last synced.
    pub(crate) fn units_done(&self) -> bool {
        self.basis.is_some() && self.dirty.is_empty()
    }

    /// The chart-level and unit findings.
    pub(crate) fn local_findings(&self) -> impl Iterator<Item = &Finding> {
        self.chart.iter().chain(self.units.values().flatten())
    }
}

fn basis(doc: &Document) -> Basis {
    Basis {
        settings: doc.settings().clone(),
        categories: doc.categories().to_vec(),
        floors: doc.floors().to_vec(),
    }
}

/// The chart-level data with these nodes only.
fn chart_only(doc: &Document, nodes: Vec<Node>) -> ChartDocument {
    ChartDocument {
        schema: doc.schema,
        name: doc.name().to_owned(),
        settings: doc.settings().clone(),
        next_oid: doc.next_oid(),
        categories: doc.categories().to_vec(),
        floors: doc.floors().to_vec(),
        nodes,
    }
}

/// The validation panel's list (07 §10).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationView {
    /// Errors first, then warnings; each by code, then node; findings of
    /// one node in the compiler's order.
    pub findings: Vec<FindingView>,
    pub errors: u32,
    pub warnings: u32,
    /// The full pass has run on the document as it is: the list is exactly
    /// the compiler's, and nothing is stale.
    pub complete: bool,
    /// Complete, without errors, and the chart encodes: publish may proceed.
    pub publishable: bool,
    /// The compiler could not encode a chart without errors (over 4 GiB).
    pub encode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingView {
    /// `"E001"` … `"W110"`, or `"unsupported"`.
    pub code: &'static str,
    /// `"error"` or `"warning"`.
    pub severity: &'static str,
    /// The node to select; `None` for settings, categories and floors.
    pub node: Option<u32>,
    pub related: Option<u32>,
    pub message: String,
    /// The row the finding names (a block row's `rid`, or a free row's id).
    pub row: Option<u32>,
    /// The seat position it names, by oid.
    pub seat: Option<u32>,
    /// From the last full pass, which the document has changed since.
    pub stale: bool,
    /// `node` as the layers panel names it: its name, label or text, or
    /// `None` and its kind (the shell then shows the kind, as the layers
    /// panel does).
    pub name: Option<String>,
    pub kind: Option<NodeKind>,
    /// The same for `related`.
    pub related_name: Option<String>,
    pub related_kind: Option<NodeKind>,
    /// What the shell needs to say what is wrong in a person's words. The
    /// labels of the row and seat it names are read for the findings shown
    /// only (`Session::finding_place`), never for the whole list.
    pub detail: FindingDetail,
}

/// A finding's particulars without ids: names, labels, keys and numbers
/// (03 §3.2). In JSON `{"kind": "duplicateLabel", "label": "A-1"}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FindingDetail {
    Schema { schema: u32, newest: u32 },
    Migration { from: u32 },
    /// The same full label is on `related`'s seat too.
    DuplicateLabel { label: String },
    LabelTooLong,
    /// The row named has no label; with a seat, that seat has none.
    EmptyLabel { seat: bool },
    NoCategory,
    UnknownCategory { key: String },
    UnknownFloor,
    UnknownIcon { key: String },
    /// A section child on another floor than its section's (names, if any).
    FloorConflict { floor: Option<String>, section_floor: Option<String> },
    DuplicateFloor { floor: Option<String> },
    DuplicateCategory { key: String },
    /// The category's label, else its key.
    CategoryColor { category: String },
    FocalPoint { floor: Option<String> },
    /// An id `related` uses too: a copy made outside the designer.
    DuplicateId,
    /// The document's field name, e.g. `seatPitch`.
    InvalidValue { field: String },
    EmptyText,
    OutOfRange,
    /// The compiler's words: `zero size`, `self-intersecting outline`,
    /// `cannot be tessellated`.
    Geometry { problem: String },
    /// More than `limit` of `what` (the compiler's words).
    TooMany { what: String, limit: u32 },
    /// A kind of object not compiled yet: `table`, `booth`, `area`, `section`.
    Unsupported { node_kind: String },
    Overlap,
    /// The floor's name; `None` for an unnamed floor or a chart without
    /// floors.
    NoFocalPoint { floor: Option<String> },
    SingleSeat,
    OutsideOutline,
    UnusedCategory { category: String },
    NoAccessibleSeat,
    SimilarColors { category: String, other: String },
    /// `ratio` as 4.5 for 4.5:1.
    LowContrast { category: String, background: String, ratio: f64 },
    /// CSS pixels at fit zoom.
    TextTooSmall { px: f64 },
    NoSectionLabel,
    ChartTooLarge { extent: u32 },
    TooManySeatsInRow { seats: u32, limit: u32 },
    TooManySeats { limit: u32 },
    SeatIds,
    RemovedOutOfRange,
    ArcRow,
    PathTooShort,
    DirectionNotForRows,
    InvalidLabelStart,
    /// The row's labels cannot be written: `zeroStep`, `startParity`,
    /// `overflow`, `outOfRange`, `customExhausted`, `tooLong`.
    Labels { problem: String },
}

/// The label of the row and seat a finding names, as a person reads them
/// on the chart, and their places counted from 1 (the row from the front,
/// the seat from the start of its row) for when a label is empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingPlace {
    pub row: Option<String>,
    pub row_number: Option<u32>,
    pub seat: Option<String>,
    pub seat_number: Option<u32>,
}

/// The layers panel's name of node `id` and its kind.
fn named(doc: &Document, id: Option<u32>) -> (Option<String>, Option<NodeKind>) {
    match id.and_then(|id| doc.node(id)) {
        Some(n) => (crate::layers::name(n), Some(NodeKind::of(n))),
        None => (None, None),
    }
}

/// A floor's name, `None` when it has none or is not listed.
fn floor_name(doc: &Document, id: u32) -> Option<String> {
    let f = doc.floors().iter().find(|f| f.id == id)?;
    (!f.name.is_empty()).then(|| f.name.clone())
}

/// A category's label, else its key.
fn category_label(doc: &Document, key: &str) -> String {
    let c = doc.categories().iter().find(|c| c.key == key);
    c.map_or_else(|| key.to_owned(), |c| c.label.clone())
}

fn label_problem(e: cavea_core::label::LabelError) -> &'static str {
    use cavea_core::label::LabelError as L;
    match e {
        L::ZeroStep => "zeroStep",
        L::StartParity => "startParity",
        L::Overflow => "overflow",
        L::OutOfRange => "outOfRange",
        L::CustomExhausted => "customExhausted",
        L::TooLong => "tooLong",
    }
}

fn finding_detail(doc: &Document, d: &Detail) -> FindingDetail {
    use FindingDetail as F;
    let s = |v: &str| v.to_owned();
    match d {
        Detail::Schema { schema } => F::Schema {
            schema: *schema,
            newest: cavea_compiler::SCHEMA,
        },
        Detail::Migration { from, .. } => F::Migration { from: *from },
        Detail::Expand(k) => match k {
            ExpandErrorKind::TooManySeatsInRow { seats, .. } => F::TooManySeatsInRow {
                seats: *seats,
                limit: cavea_core::limits::MAX_SEATS_PER_ROW,
            },
            ExpandErrorKind::TooManyObjects => F::TooManySeats {
                limit: cavea_core::limits::MAX_OBJECTS,
            },
            ExpandErrorKind::OidRuns { .. } => F::SeatIds,
            ExpandErrorKind::RemovedOutOfRange { .. } => F::RemovedOutOfRange,
            ExpandErrorKind::InvalidNumber { field } => F::InvalidValue { field: s(field) },
            ExpandErrorKind::ArcRow { .. } => F::ArcRow,
            ExpandErrorKind::PathTooShort => F::PathTooShort,
            ExpandErrorKind::DirectionNotForRows => F::DirectionNotForRows,
            ExpandErrorKind::InvalidLabelStart => F::InvalidLabelStart,
            ExpandErrorKind::Label { error, .. } => F::Labels {
                problem: s(label_problem(*error)),
            },
        },
        Detail::DuplicateLabel { label, .. } => F::DuplicateLabel {
            label: s(label.as_str()),
        },
        Detail::LabelTooLong => F::LabelTooLong,
        Detail::EmptyLabel { position, .. } => F::EmptyLabel {
            seat: position.is_some(),
        },
        Detail::NoCategory => F::NoCategory,
        Detail::UnknownCategory { key } => F::UnknownCategory { key: key.clone() },
        Detail::UnknownFloor { .. } => F::UnknownFloor,
        Detail::UnknownIcon { key } => F::UnknownIcon { key: key.clone() },
        Detail::FloorConflict {
            floor,
            section_floor,
        } => F::FloorConflict {
            floor: floor_name(doc, *floor),
            section_floor: floor_name(doc, *section_floor),
        },
        Detail::DuplicateFloor { floor } => F::DuplicateFloor {
            floor: floor_name(doc, *floor),
        },
        Detail::DuplicateCategory { key } => F::DuplicateCategory { key: key.clone() },
        Detail::CategoryColor { key } => F::CategoryColor {
            category: category_label(doc, key),
        },
        Detail::FocalPoint { floor } => F::FocalPoint {
            floor: floor_name(doc, *floor),
        },
        Detail::DuplicateId { .. } => F::DuplicateId,
        Detail::InvalidValue { field } => F::InvalidValue { field: s(field) },
        Detail::EmptyText => F::EmptyText,
        Detail::OutOfRange => F::OutOfRange,
        Detail::Geometry { problem } => F::Geometry { problem: s(problem) },
        Detail::TooMany { what, limit } => F::TooMany {
            what: s(what),
            limit: *limit,
        },
        Detail::UnsupportedNode { kind } => F::Unsupported { node_kind: s(kind) },
        Detail::Overlap { .. } => F::Overlap,
        Detail::NoFocalPoint { floor } => F::NoFocalPoint {
            floor: floor.and_then(|f| floor_name(doc, f)),
        },
        Detail::SingleSeat { .. } => F::SingleSeat,
        Detail::OutsideOutline { .. } => F::OutsideOutline,
        Detail::UnusedCategory { key } => F::UnusedCategory {
            category: category_label(doc, key),
        },
        Detail::NoAccessibleSeat => F::NoAccessibleSeat,
        Detail::SimilarColors { key, other } => F::SimilarColors {
            category: category_label(doc, other),
            other: category_label(doc, key),
        },
        Detail::LowContrast {
            key,
            background,
            ratio,
        } => F::LowContrast {
            category: category_label(doc, key),
            background: s(background),
            ratio: f64::from(*ratio) / 100.0,
        },
        Detail::TextTooSmall { px } => F::TextTooSmall {
            px: f64::from(*px) / 100.0,
        },
        Detail::NoSectionLabel => F::NoSectionLabel,
        Detail::ChartTooLarge { extent } => F::ChartTooLarge { extent: *extent },
    }
}

/// The labels of row `rid` of node `id` and of its seat `oid` (by the
/// chart's label template, as buyers read it), from one expansion of that
/// node: for the findings the validation panel shows.
pub(crate) fn place(doc: &Document, id: u32, rid: Option<u32>, oid: Option<u32>) -> FindingPlace {
    use cavea_compiler::Expansion;
    let mut out = FindingPlace::default();
    let overflow = doc.settings().alpha_overflow;
    let mut e = Expansion::new();
    let Some(node) = doc.node(id) else {
        return out;
    };
    let ok = match node {
        Node::RowBlock(b) => e.push_row_block(b, overflow).is_ok(),
        Node::Row(r) => e.push_row(r, overflow).is_ok(),
        _ => false,
    };
    if !ok {
        return out;
    }
    let found = rid.and_then(|rid| e.rows.iter().enumerate().find(|(_, r)| r.rid == rid));
    if let Some((i, row)) = found {
        out.row_number = u32::try_from(i + 1).ok();
        out.row = Some(row.label.as_str().to_owned()).filter(|l| !l.is_empty());
    }
    if let Some(oid) = oid
        && let Some((at, seat)) = e.seats.iter().enumerate().find(|(_, s)| s.oid == oid)
        && let Some(row) = e.rows.get(seat.row as usize)
    {
        let first = row.first_seat as usize;
        out.seat_number = u32::try_from(at.saturating_sub(first) + 1).ok();
        out.seat = crate::numbering::sample(doc, id, node, oid, overflow);
    }
    out
}

fn finding_view(doc: &Document, f: &Finding, stale: bool) -> FindingView {
    let (row, position) = match &f.detail {
        Detail::EmptyLabel { rid, position } => (Some(*rid), position.map(u32::from)),
        Detail::SingleSeat { rid } => (Some(*rid), None),
        Detail::OutsideOutline { rid, position, .. } => (Some(*rid), Some(u32::from(*position))),
        Detail::Expand(
            ExpandErrorKind::TooManySeatsInRow { rid, .. }
            | ExpandErrorKind::OidRuns { rid }
            | ExpandErrorKind::ArcRow { rid }
            | ExpandErrorKind::Label { rid, .. },
        ) => (Some(*rid), None),
        Detail::Expand(ExpandErrorKind::RemovedOutOfRange { rid, .. }) => (Some(*rid), None),
        _ => (None, None),
    };
    let seat = match (f.node.and_then(|n| doc.node(n)), row, position) {
        (Some(Node::RowBlock(b)), Some(rid), Some(p)) => b
            .rows
            .iter()
            .find(|r| r.rid == rid)
            .and_then(|r| expand_runs(r, b.id).ok())
            .and_then(|oids| oids.get(p as usize).copied()),
        (Some(Node::Row(r)), _, Some(p)) => {
            let runs = r.oids.iter().flat_map(|&[a, n]| a..a.saturating_add(n));
            runs.clone().nth(p as usize)
        }
        _ => None,
    };
    let (name, kind) = named(doc, f.node);
    let (related_name, related_kind) = named(doc, f.related());
    FindingView {
        code: f.code.as_str(),
        severity: f.severity().as_str(),
        node: f.node,
        related: f.related(),
        message: f.message(),
        row,
        seat,
        stale,
        name,
        kind,
        related_name,
        related_kind,
        detail: finding_detail(doc, &f.detail),
    }
}

/// A node for the canvas to mark, with the worst severity of its findings.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub node: u32,
    /// `"error"` or `"warning"`.
    pub severity: &'static str,
    pub bounds: Option<Rect>,
}

/// The markers for the nodes of `view` that `shown` accepts, by node id.
pub(crate) fn markers(
    view: &ValidationView,
    index: &SpatialIndex,
    shown: impl Fn(u32) -> bool,
) -> Vec<Marker> {
    let mut worst: BTreeMap<u32, &'static str> = BTreeMap::new();
    for f in &view.findings {
        if let Some(n) = f.node.filter(|&n| shown(n)) {
            let slot = worst.entry(n).or_insert(f.severity);
            if f.severity == "error" {
                *slot = "error";
            }
        }
    }
    worst
        .into_iter()
        .map(|(node, severity)| Marker {
            node,
            severity,
            bounds: index.bounds(node),
        })
        .collect()
}
