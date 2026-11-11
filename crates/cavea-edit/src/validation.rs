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
    FindingView {
        code: f.code.as_str(),
        severity: f.severity().as_str(),
        node: f.node,
        related: f.related(),
        message: f.message(),
        row,
        seat,
        stale,
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
