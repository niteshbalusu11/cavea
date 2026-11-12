//! The numbering panel's rules (`docs/07-designer.md` §6): row and seat
//! labels for the selected blocks and rows, in the document's own label
//! rules (03 §2.5), with a preview that changes nothing.
//!
//! [`renumbered`] computes each target node as it would be after the
//! numbering; applying it is a composite of existing commands, and the
//! preview expands the same nodes with the compiler's own expansion, so the
//! labels previewed are the labels applied.

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::{
    AlphaOverflow, LabelScheme, LabelStart, Node, RowDirection, RowLabels, SeatDirection,
    SeatLabels,
};
use cavea_compiler::{Expansion, write_full_label};
use cavea_core::label::LabelBuf;
use cavea_core::limits::MAX_LABEL_LEN;
use serde::{Deserialize, Serialize};

use crate::command::{ChartProperty, Command, CommandError, Property, SeatEdit};
use crate::document::Document;
use crate::index::P;
use crate::selection::{Level, Selection, View, floor_of, pickable};

/// Entries of a custom or skip list (03 §2.5 sets no bound; the session
/// refuses longer lists rather than copy them to every selected block).
pub const MAX_LIST: usize = 65_535;

/// What the numbering panel sets. Seat `ltr` and `rtl` are as seen facing
/// the focal point of the node's floor (07 §6): a node whose seats run the
/// other way from there stores the other one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Numbering {
    /// Row labels for every selected row block; `null` keeps each one's.
    #[serde(default)]
    pub rows: Option<RowLabels>,
    /// Seat labels for every selected row block and row; `null` keeps each
    /// one's.
    #[serde(default)]
    pub seats: Option<SeatLabels>,
    /// After Z: the chart's `settings.alphaOverflow`, which every block
    /// follows (the schema has no per-block field).
    #[serde(default)]
    pub after_z: Option<AlphaOverflow>,
    /// Seat numbering continues through the selected nodes: those listed
    /// here first, in this order, then the rest in document order. Each
    /// node then starts where the one before it ends (03 §2.5 `start`):
    /// after its last position with `continuous`, else after its longest
    /// row. A custom list continues where the previous node left it.
    #[serde(default)]
    pub across: Option<Vec<u32>>,
    /// Keep row and seat label overrides. Without it, renumbering rows
    /// clears the block's row label overrides, and renumbering seats clears
    /// its seats' label overrides (their other fields stay).
    #[serde(default)]
    pub keep_overrides: bool,
}

/// How a node's left-to-right numbering (`ltr`, from the left end in its
/// own space) reads for a viewer facing the focal point of its floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Facing {
    /// `ltr` reads left to right.
    Forward,
    /// `ltr` reads right to left: the node faces away.
    Reversed,
    /// No focal point, or the node is edge-on to it: `ltr` is the node's own.
    Unknown,
}

/// One value shared by every selected node, or `"mixed"`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Shared<T> {
    Same(T),
    Mixed,
}

pub(crate) fn shared<T: PartialEq>(mut values: impl Iterator<Item = T>) -> Option<Shared<T>> {
    let first = values.next()?;
    Some(if values.all(|v| v == first) {
        Shared::Same(first)
    } else {
        Shared::Mixed
    })
}

/// The numbering panel's view of the selection.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NumberingView {
    pub nodes: Vec<NumberedNode>,
    /// Each field of the row blocks' row labels; `None` without row blocks.
    pub rows: Option<RowLabelsView>,
    /// Each field of the seat labels, `ltr` and `rtl` as seen facing the
    /// focal point; `None` without targets.
    pub seats: Option<SeatLabelsView>,
    pub after_z: AlphaOverflow,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NumberedNode {
    pub id: u32,
    /// As the layers panel names it.
    pub name: String,
    /// A row block (`true`) or a free-form row.
    pub block: bool,
    pub facing: Facing,
    /// Its seats use `center-out` or `serpentine`, which have no mirrored
    /// form, and it faces away: they read mirrored, and only renumbering in
    /// another direction changes that.
    pub mirrored: bool,
    /// Rows with a label override, and seats with a label override.
    pub row_overrides: u32,
    pub seat_overrides: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowLabelsView {
    pub scheme: Shared<LabelScheme>,
    pub start: Shared<Option<LabelStart>>,
    pub step: Shared<u32>,
    pub skip: Shared<Vec<String>>,
    pub direction: Shared<RowDirection>,
    pub prefix: Shared<String>,
    pub suffix: Shared<String>,
    pub custom: Shared<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatLabelsView {
    pub scheme: Shared<LabelScheme>,
    pub start: Shared<Option<LabelStart>>,
    pub step: Shared<u32>,
    pub direction: Shared<SeatDirection>,
    pub continuous: Shared<bool>,
    pub prefix: Shared<String>,
    pub suffix: Shared<String>,
    pub custom: Shared<Vec<String>>,
}

/// A seat position of a row block or row: the hovered seat whose full
/// label the numbering panel samples (07 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatRef {
    pub node: u32,
    pub oid: u32,
}

/// The labels a numbering would give, without applying it: per row, its
/// label and each present seat's label at its world position.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub rows: Vec<PreviewRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub node: u32,
    /// The row's `rid`; a free-form row's node id.
    pub rid: u32,
    pub label: String,
    pub seats: Vec<PreviewSeat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSeat {
    pub oid: u32,
    pub at: P,
    pub label: String,
}

/// The row blocks and rows the numbering acts on, in document order: the
/// selected ones that can be picked, a selected section's, or the one whose
/// rows or seats are selected.
pub(crate) fn targets(view: &View<'_>, sel: &Selection) -> Vec<u32> {
    let doc = view.doc;
    let numbered = |id: u32| {
        matches!(doc.node(id), Some(Node::RowBlock(_) | Node::Row(_)))
            && pickable(doc, view.floor, id)
    };
    let wanted = |id: u32| match sel.level() {
        Level::Top | Level::Section(_) => {
            let own = sel.items().contains(&id);
            own || doc.parent(id).is_some_and(|s| sel.items().contains(&s))
        }
        Level::Rows(h) | Level::Seats(h) => h == id,
    };
    let mut out = Vec::new();
    for &top in doc.children(None) {
        for id in std::iter::once(top).chain(doc.children(Some(top)).iter().copied()) {
            if numbered(id) && wanted(id) {
                out.push(id);
            }
        }
    }
    out
}

/// Which way a node's own left-to-right runs for a viewer at the node
/// facing its floor's focal point: compared with the viewer's right hand,
/// `(−v.y, v.x)` for the view direction `v` (y down), the direction from its
/// first row's first position to its last.
pub(crate) fn facing(view: &View<'_>, id: u32) -> Facing {
    let doc = view.doc;
    let floor = floor_of(doc, id);
    let focal = doc
        .floors()
        .iter()
        .find(|f| Some(f.id) == floor)
        .and_then(|f| f.focal_point);
    let seats = view.index.seats(id);
    let row0 = seats.first().map(|s| s.row);
    let first_row = seats.iter().take_while(|s| Some(s.row) == row0);
    let ends = (seats.first(), first_row.last());
    let (Some(f), (Some(a), Some(b)), Some(bounds)) = (focal, ends, view.index.bounds(id)) else {
        return Facing::Unknown;
    };
    let world = |p| view.index.to_world(id, p);
    let (Some(a), Some(b)) = (world(a.pos), world(b.pos)) else {
        return Facing::Unknown;
    };
    let d = if a == b {
        // One position: the node's own +x.
        match (world([0.0, 0.0]), world([1.0, 0.0])) {
            (Some(o), Some(x)) => [x[0] - o[0], x[1] - o[1]],
            _ => return Facing::Unknown,
        }
    } else {
        [b[0] - a[0], b[1] - a[1]]
    };
    let c = [
        (bounds.min[0] + bounds.max[0]) / 2.0,
        (bounds.min[1] + bounds.max[1]) / 2.0,
    ];
    let v = [f[0] - c[0], f[1] - c[1]];
    let dot = d[0] * -v[1] + d[1] * v[0];
    let scale = d[0].hypot(d[1]) * v[0].hypot(v[1]);
    if !dot.is_finite() || dot.abs() <= scale * 1e-9 {
        Facing::Unknown
    } else if dot > 0.0 {
        Facing::Forward
    } else {
        Facing::Reversed
    }
}

/// `ltr` and `rtl` as stored for a node facing `facing`, from or to how a
/// viewer facing the focal point reads them (the mapping is its own inverse).
fn as_seen(direction: SeatDirection, facing: Facing) -> SeatDirection {
    match (direction, facing) {
        (SeatDirection::Ltr, Facing::Reversed) => SeatDirection::Rtl,
        (SeatDirection::Rtl, Facing::Reversed) => SeatDirection::Ltr,
        (d, _) => d,
    }
}

fn seat_labels(node: &Node) -> Option<&SeatLabels> {
    match node {
        Node::RowBlock(b) => Some(&b.seat_labels),
        Node::Row(r) => Some(&r.seat_labels),
        _ => None,
    }
}

pub(crate) fn view(view: &View<'_>, sel: &Selection) -> NumberingView {
    let doc = view.doc;
    let ids = targets(view, sel);
    let nodes: Vec<(u32, &Node, Facing)> = ids
        .iter()
        .filter_map(|&id| Some((id, doc.node(id)?, facing(view, id))))
        .collect();
    let blocks: Vec<&RowLabels> = nodes
        .iter()
        .filter_map(|(_, n, _)| match n {
            Node::RowBlock(b) => Some(&b.row_labels),
            _ => None,
        })
        .collect();
    let seats: Vec<(&SeatLabels, Facing)> = nodes
        .iter()
        .filter_map(|(_, n, f)| Some((seat_labels(n)?, *f)))
        .collect();
    let rows = (|| {
        let each = || blocks.iter();
        Some(RowLabelsView {
            scheme: shared(each().map(|r| r.scheme))?,
            start: shared(each().map(|r| r.start.clone()))?,
            step: shared(each().map(|r| r.step))?,
            skip: shared(each().map(|r| r.skip.clone()))?,
            direction: shared(each().map(|r| r.direction))?,
            prefix: shared(each().map(|r| r.prefix.clone()))?,
            suffix: shared(each().map(|r| r.suffix.clone()))?,
            custom: shared(each().map(|r| r.custom.clone()))?,
        })
    })();
    let seat_view = (|| {
        let each = || seats.iter();
        Some(SeatLabelsView {
            scheme: shared(each().map(|(s, _)| s.scheme))?,
            start: shared(each().map(|(s, _)| s.start.clone()))?,
            step: shared(each().map(|(s, _)| s.step))?,
            direction: shared(each().map(|(s, f)| as_seen(s.direction, *f)))?,
            continuous: shared(each().map(|(s, _)| s.continuous))?,
            prefix: shared(each().map(|(s, _)| s.prefix.clone()))?,
            suffix: shared(each().map(|(s, _)| s.suffix.clone()))?,
            custom: shared(each().map(|(s, _)| s.custom.clone()))?,
        })
    })();
    let nodes = nodes
        .iter()
        .map(|&(id, node, facing)| {
            let (row_overrides, overrides) = match node {
                Node::RowBlock(b) => (
                    b.rows.iter().filter(|r| r.label.is_some()).count(),
                    &b.seat_overrides,
                ),
                Node::Row(r) => (0, &r.seat_overrides),
                _ => (0, &Default::default()),
            };
            let seat_overrides = overrides.values().filter(|o| o.label.is_some()).count();
            let unmirrored = seat_labels(node).is_some_and(|s| {
                matches!(
                    s.direction,
                    SeatDirection::CenterOut | SeatDirection::Serpentine
                )
            });
            NumberedNode {
                id,
                name: crate::naming::shown(doc, id).unwrap_or_default(),
                block: matches!(node, Node::RowBlock(_)),
                facing,
                mirrored: unmirrored && facing == Facing::Reversed,
                row_overrides: row_overrides as u32,
                seat_overrides: seat_overrides as u32,
            }
        })
        .collect();
    NumberingView {
        nodes,
        rows,
        seats: seat_view,
        after_z: doc.settings().alpha_overflow,
    }
}

/// Every label part from the user is at most 64 bytes, and lists at most
/// [`MAX_LIST`] entries: refused, not reported, since one value is copied to
/// every selected node.
fn check(n: &Numbering) -> Result<(), CommandError> {
    let part = |s: &str| s.len() <= MAX_LABEL_LEN;
    let start = |s: &Option<LabelStart>| match s {
        Some(LabelStart::Text(t)) => part(t),
        _ => true,
    };
    let list = |l: &[String]| l.len() <= MAX_LIST && l.iter().all(|s| part(s));
    let rows_ok = n.rows.as_ref().is_none_or(|r| {
        part(&r.prefix) && part(&r.suffix) && start(&r.start) && list(&r.skip) && list(&r.custom)
    });
    let seats_ok = n
        .seats
        .as_ref()
        .is_none_or(|s| part(&s.prefix) && part(&s.suffix) && start(&s.start) && list(&s.custom));
    let tables = n.seats.as_ref().is_some_and(|s| {
        matches!(
            s.direction,
            SeatDirection::Clockwise | SeatDirection::Counterclockwise
        )
    });
    if tables {
        return Err(CommandError::InvalidValue("seatLabels.direction"));
    }
    if rows_ok && seats_ok && n.across.as_ref().is_none_or(|a| a.len() <= MAX_LIST) {
        Ok(())
    } else {
        Err(CommandError::Limit("label bytes"))
    }
}

/// The targets in numbering order: `across` first, then document order.
fn ordered(targets: &[u32], across: Option<&[u32]>) -> Vec<u32> {
    let listed = across.unwrap_or_default();
    let mut left: BTreeSet<u32> = targets.iter().copied().collect();
    let mut out: Vec<u32> = Vec::with_capacity(targets.len());
    for &id in listed.iter().chain(targets) {
        if left.remove(&id) {
            out.push(id);
        }
    }
    out
}

/// Each target node as the numbering leaves it, in document order.
/// Mistakes in the rules themselves (a step of 0, a custom list too short,
/// duplicate labels) are not refused: validation reports them (03 §3.2).
pub(crate) fn renumbered(
    view: &View<'_>,
    targets: &[u32],
    n: &Numbering,
) -> Result<Vec<(u32, Node)>, CommandError> {
    check(n)?;
    let doc = view.doc;
    let overflow = n.after_z.unwrap_or(doc.settings().alpha_overflow);
    let mut out = Vec::new();
    // Where the next node's seat numbering starts, under `across`.
    let mut next: Option<(Option<LabelStart>, Vec<String>)> = None;
    for id in ordered(targets, n.across.as_deref()) {
        let Some(mut node) = doc.node(id).cloned() else {
            continue;
        };
        let clear = !n.keep_overrides;
        if let (Node::RowBlock(b), Some(rows)) = (&mut node, &n.rows) {
            b.row_labels = rows.clone();
            if clear {
                b.rows.iter_mut().for_each(|r| r.label = None);
            }
        }
        if let Some(seats) = &n.seats {
            let mut sl = seats.clone();
            sl.direction = as_seen(sl.direction, facing(view, id));
            if let Some((start, custom)) = next.take() {
                sl.start = start;
                sl.custom = custom;
            }
            let (labels, overrides) = match &mut node {
                Node::RowBlock(b) => (&mut b.seat_labels, &mut b.seat_overrides),
                Node::Row(r) => (&mut r.seat_labels, &mut r.seat_overrides),
                _ => continue,
            };
            *labels = sl;
            if clear {
                // As `editSeats label null` on the node's seats: an override
                // left empty goes; one for an oid the node lacks is kept, as
                // `editSeats` cannot name it (and it labels no seat).
                let oids: BTreeSet<u32> = view.index.seats(id).iter().map(|s| s.oid).collect();
                overrides.retain(|oid, o| {
                    if !oids.contains(oid) {
                        return true;
                    }
                    let had = o.label.take().is_some();
                    !had || !o.flags.is_empty() || o.category.is_some() || o.nudge.is_some()
                });
            }
            if n.across.is_some() {
                next = Some(continuation(&node, overflow, seats));
            }
        }
        out.push((id, node));
    }
    // Document order, whatever the numbering order.
    let place: BTreeMap<u32, usize> = targets.iter().enumerate().map(|(i, &t)| (t, i)).collect();
    out.sort_by_key(|(id, _)| place.get(id).copied());
    Ok(out)
}

/// Where numbering continues after `node` (`across`): its rule's value one
/// past its last ordinal. With a rule that does not expand, the next node
/// starts as the panel asked, and validation reports both.
fn continuation(
    node: &Node,
    overflow: AlphaOverflow,
    asked: &SeatLabels,
) -> (Option<LabelStart>, Vec<String>) {
    let fallback = (asked.start.clone(), asked.custom.clone());
    let (sl, used) = match node {
        Node::RowBlock(b) => {
            let total: u32 = b.rows.iter().map(|r| r.seats).sum();
            let longest = b.rows.iter().map(|r| r.seats).max().unwrap_or(0);
            let used = if b.seat_labels.continuous {
                total
            } else {
                longest
            };
            (&b.seat_labels, used)
        }
        Node::Row(r) => (&r.seat_labels, r.seats),
        _ => return fallback,
    };
    if sl.scheme == LabelScheme::Custom {
        let rest = sl.custom.get(used as usize..).unwrap_or_default();
        return (None, rest.to_vec());
    }
    // The seat rule alone decides: row labels that do not expand (which
    // validation reports) do not break the chain.
    let mut plain = node.clone();
    if let Node::RowBlock(b) = &mut plain {
        b.row_labels = RowLabels {
            scheme: LabelScheme::Numeric,
            ..RowLabels::default()
        };
        b.rows.iter_mut().for_each(|r| r.label = None);
    }
    let mut e = Expansion::new();
    let ok = match &plain {
        Node::RowBlock(b) => e.push_row_block(b, overflow).is_ok(),
        Node::Row(r) => e.push_row(r, overflow).is_ok(),
        _ => false,
    };
    let value = e
        .rows
        .first()
        .filter(|_| ok)
        .map(|r| r.seat_rule.value(used));
    match value {
        Some(Ok(v)) => (Some(LabelStart::Number(v)), asked.custom.clone()),
        _ => fallback,
    }
}

/// The commands that turn the document's targets into `nodes`, and the
/// chart setting for After Z: one composite, one undo entry.
pub(crate) fn command(doc: &Document, nodes: &[(u32, Node)], n: &Numbering) -> Option<Command> {
    let mut commands = Vec::new();
    if let Some(o) = n.after_z
        && o != doc.settings().alpha_overflow
    {
        let mut settings = doc.settings().clone();
        settings.alpha_overflow = o;
        commands.push(Command::SetChart {
            property: ChartProperty::Settings(settings),
        });
    }
    for (id, new) in nodes {
        let (id, Some(old)) = (*id, doc.node(*id)) else {
            continue;
        };
        if let (Node::RowBlock(a), Node::RowBlock(b)) = (old, new) {
            if a.row_labels != b.row_labels {
                let property = Property::RowLabels(Box::new(b.row_labels.clone()));
                commands.push(Command::SetProperty { id, property });
            }
            for (ra, rb) in a.rows.iter().zip(&b.rows) {
                if ra.label != rb.label {
                    let (rid, label) = (rb.rid, rb.label.clone());
                    commands.push(Command::SetRowLabel { id, rid, label });
                }
            }
        }
        let (Some(sa), Some(sb)) = (seat_labels(old), seat_labels(new)) else {
            continue;
        };
        if sa != sb {
            let property = Property::SeatLabels(Box::new(sb.clone()));
            commands.push(Command::SetProperty { id, property });
        }
        let overrides = |n: &Node| match n {
            Node::RowBlock(b) => b.seat_overrides.clone(),
            Node::Row(r) => r.seat_overrides.clone(),
            _ => Default::default(),
        };
        let (oa, ob) = (overrides(old), overrides(new));
        let cleared: Vec<u32> = oa
            .iter()
            .filter(|(k, o)| o.label.is_some() && ob.get(k).is_none_or(|x| x.label.is_none()))
            .map(|(&k, _)| k)
            .collect();
        if !cleared.is_empty() {
            let edit = SeatEdit::Label { label: None };
            commands.push(Command::EditSeats {
                id,
                oids: cleared,
                edit,
            });
        }
    }
    (!commands.is_empty()).then_some(Command::Composite { commands })
}

/// The labels of `nodes` at their seats' world positions now (renumbering
/// moves no seat).
pub(crate) fn preview(view: &View<'_>, nodes: &[(u32, Node)], overflow: AlphaOverflow) -> Preview {
    let mut rows = Vec::new();
    for (id, node) in nodes {
        let mut e = Expansion::new();
        let ok = match node {
            Node::RowBlock(b) => e.push_row_block(b, overflow).is_ok(),
            Node::Row(r) => e.push_row(r, overflow).is_ok(),
            _ => false,
        };
        let Some(place) = view.index.place(*id).filter(|_| ok) else {
            continue;
        };
        for row in &e.rows {
            let first = row.first_seat as usize;
            let seats = e
                .seats
                .get(first..first + usize::from(row.seat_count))
                .unwrap_or_default();
            let seats = seats
                .iter()
                .filter(|s| !s.removed)
                .filter_map(|s| {
                    let mut label = LabelBuf::new();
                    row.write_seat_label(s, &mut label).ok()?;
                    Some(PreviewSeat {
                        oid: s.oid,
                        at: place.apply(s.pos),
                        label: label.as_str().to_owned(),
                    })
                })
                .collect();
            rows.push(PreviewRow {
                node: *id,
                rid: row.rid,
                label: row.label.as_str().to_owned(),
                seats,
            });
        }
    }
    Preview { rows }
}

/// The full label of seat `oid` of node `id`, given as `node` (the
/// document's, or the preview's),
/// under the chart's label template (03 §3.1.1), or `None` if it does not
/// expand or is past 64 bytes.
pub(crate) fn sample(
    doc: &Document,
    id: u32,
    node: &Node,
    oid: u32,
    overflow: AlphaOverflow,
) -> Option<String> {
    let mut e = Expansion::new();
    match node {
        Node::RowBlock(b) => e.push_row_block(b, overflow).ok()?,
        Node::Row(r) => e.push_row(r, overflow).ok()?,
        _ => return None,
    }
    let seat = e.seats.iter().find(|s| s.oid == oid)?;
    let row = e.rows.get(seat.row as usize)?;
    let mut text = LabelBuf::new();
    row.write_seat_label(seat, &mut text).ok()?;
    let section = doc
        .parent(id)
        .and_then(|s| doc.node(s))
        .and_then(|s| match s {
            Node::Section(s) => s.label.clone(),
            _ => None,
        })
        .unwrap_or_default();
    let mut full = LabelBuf::new();
    let template = &doc.settings().label_template;
    write_full_label(
        template,
        &section,
        row.label.as_str(),
        text.as_str(),
        &mut full,
    )
    .ok()?;
    Some(full.as_str().to_owned())
}
