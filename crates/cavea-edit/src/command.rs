//! Commands: the designer's edit vocabulary (`docs/07-designer.md` §2).
//!
//! A [`Command`] is plain data with a stable JSON form, tagged by `op`. It is
//! translated into [`Op`]s against the current document and applied one op at
//! a time; each op returns its inverse. If any step fails, the ops applied so
//! far are undone and the command returns the error, leaving the document as
//! it was. New ids (nodes, rows, seats) are allocated here, never taken from
//! the command, so a command from outside cannot hand out an id twice.

use std::collections::BTreeMap;
use std::fmt;
use std::mem::replace;

use cavea_compiler::document::{
    Align, Category, Curve, Floor, Node, Outline, RowLabels, SeatLabels, SeatOverride, Settings,
};
use cavea_core::limits::{MAX_CATEGORIES, MAX_FLOORS, MAX_OBJECTS, MAX_SEATS_PER_ROW};
use serde::{Deserialize, Serialize};

use crate::document::{Document, each_node, node_id};
use crate::op::Op;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Command {
    /// Adds `node` (with its children, for a section) at `index` in
    /// `parent`'s children. Every id in it is replaced by a fresh one, and
    /// seat overrides follow their seats.
    CreateNode {
        parent: Option<u32>,
        index: u32,
        node: Box<Node>,
    },
    /// Deletes the nodes and their subtrees.
    DeleteNodes {
        ids: Vec<u32>,
    },
    /// Adds `delta` to each node's origin (in its parent's space).
    MoveNodes {
        ids: Vec<u32>,
        delta: [f64; 2],
    },
    /// Sets each node's origin and rotation.
    TransformNodes {
        nodes: Vec<NodeTransform>,
    },
    /// Moves a node to `index` in `parent`'s children, counted after it has
    /// left its current place: reparenting and reordering.
    PlaceNode {
        id: u32,
        parent: Option<u32>,
        index: u32,
    },
    SetProperty {
        id: u32,
        property: Property,
    },
    SetChart {
        property: ChartProperty,
    },
    AddCategory {
        index: u32,
        category: Category,
    },
    /// Replaces the category `key`; a new key is carried to every reference.
    UpdateCategory {
        key: String,
        category: Category,
    },
    /// Removes the category `key`; references move to `replacement`, or are
    /// cleared (inherit) when it is `null`.
    RemoveCategory {
        key: String,
        replacement: Option<String>,
    },
    /// Adds a floor; its `id` is assigned (one above the largest).
    AddFloor {
        index: u32,
        floor: Floor,
    },
    /// Replaces the floor with the same `id`.
    UpdateFloor {
        floor: Floor,
    },
    /// Removes a floor; nodes on it move to `replacement`, or to the default
    /// floor when it is `null`.
    RemoveFloor {
        id: u32,
        replacement: Option<u32>,
    },
    /// Applies the commands in order, as one undo step.
    Composite {
        commands: Vec<Command>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeTransform {
    pub id: u32,
    pub origin: [f64; 2],
    pub rotation: f64,
}

/// A property the inspector edits, with its value: `{"field": ..., "value": ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "field", content = "value", rename_all = "camelCase")]
pub enum Property {
    /// Any node.
    Name(Option<String>),
    Floor(Option<u32>),
    Locked(bool),
    Hidden(bool),
    /// Section, row, table, booth, area.
    Label(Option<String>),
    /// Section, row block, row, table, booth, area.
    Category(Option<String>),
    /// Section.
    Priority(f64),
    Entrance(Option<String>),
    Outline(Outline),
    /// Row block and row.
    SeatPitch(f64),
    /// Row block.
    RowPitch(f64),
    Curve(Curve),
    Align(Align),
    Stagger(f64),
    Skew(f64),
    RowLabels(Box<RowLabels>),
    /// Row block, row, table.
    SeatLabels(Box<SeatLabels>),
    /// Shape, text, icon, image: the whole node, of the same type; its id is
    /// kept.
    Decoration(Box<Node>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "field", content = "value", rename_all = "camelCase")]
pub enum ChartProperty {
    Name(String),
    Settings(Settings),
}

/// Why a command, undo or redo was refused. The document is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    MissingNode(u32),
    NotASection(u32),
    /// The parent is the node itself or inside it.
    Cycle {
        id: u32,
        parent: u32,
    },
    /// Only top-level sections hold children, and sections are top-level.
    Placement,
    IndexOutOfRange,
    NotApplicable {
        id: u32,
        field: &'static str,
    },
    /// A number that is not finite, or would make the document unreadable.
    InvalidValue(&'static str),
    /// A limit of `docs/02-domain-model.md` §14.
    Limit(&'static str),
    IdsExhausted,
    DuplicateId(u32),
    DuplicateCategory(String),
    MissingCategory(String),
    MissingFloor(u32),
    /// Undo, redo or a new gesture while a gesture is open.
    GestureOpen,
    NoGesture,
    /// An inconsistency in the editor itself.
    Internal,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingNode(id) => write!(f, "no node {id}"),
            Self::NotASection(id) => write!(f, "node {id} is not a section"),
            Self::Cycle { id, parent } => write!(f, "node {parent} is inside node {id}"),
            Self::Placement => f.write_str("only top-level sections hold nodes"),
            Self::IndexOutOfRange => f.write_str("index out of range"),
            Self::NotApplicable { id, field } => write!(f, "node {id} has no {field}"),
            Self::InvalidValue(field) => write!(f, "invalid {field}"),
            Self::Limit(what) => write!(f, "too many {what}"),
            Self::IdsExhausted => f.write_str("no ids left"),
            Self::DuplicateId(id) => write!(f, "id {id} is taken"),
            Self::DuplicateCategory(key) => write!(f, "category {key:?} exists"),
            Self::MissingCategory(key) => write!(f, "no category {key:?}"),
            Self::MissingFloor(id) => write!(f, "no floor {id}"),
            Self::GestureOpen => f.write_str("a gesture is open"),
            Self::NoGesture => f.write_str("no gesture is open"),
            Self::Internal => f.write_str("internal editor error"),
        }
    }
}

impl std::error::Error for CommandError {}

type Result<T, E = CommandError> = std::result::Result<T, E>;

impl Document {
    /// Applies `command`, pushing the inverse of every op applied onto
    /// `inverse` and the id of every node created onto `created`. On error the
    /// caller undoes `inverse`.
    pub(crate) fn execute(
        &mut self,
        command: Command,
        inverse: &mut Vec<Op>,
        created: &mut Vec<u32>,
    ) -> Result<()> {
        let mut run = |doc: &mut Self, op: Op| -> Result<()> {
            inverse.push(doc.apply_op(op).map_err(|f| f.1)?);
            Ok(())
        };
        match command {
            Command::CreateNode {
                parent,
                index,
                mut node,
            } => {
                let section = matches!(*node, Node::Section(_));
                self.check_placement(parent, section)?;
                if let Node::Section(s) = &*node
                    && s.children.iter().any(|c| matches!(c, Node::Section(_)))
                {
                    return Err(CommandError::Placement);
                }
                // The chart's objects so far count against the limit too.
                let mut objects = self.slots.values().map(|s| own_objects(&s.node)).sum();
                let mut ids = 0;
                census(&node, &mut ids, &mut objects)?;
                if !reparses(&node) {
                    return Err(CommandError::InvalidValue("node"));
                }
                let mut next = self.allocate(ids).ok_or(CommandError::IdsExhausted)?;
                renumber(&mut node, &mut next);
                created.push(node_id(&node));
                let index = index as usize;
                run(
                    self,
                    Op::Insert {
                        parent,
                        index,
                        node,
                    },
                )?;
                self.raise_next_oid(next, run)
            }
            Command::DeleteNodes { mut ids } => {
                ids.sort_unstable();
                ids.dedup();
                for &id in &ids {
                    if !self.slots.contains_key(&id) {
                        return Err(CommandError::MissingNode(id));
                    }
                }
                // A node inside another one being deleted goes with it.
                let inside = |doc: &Self, id: u32| {
                    let mut up = doc.parent(id);
                    while let Some(a) = up {
                        if ids.binary_search(&a).is_ok() {
                            return true;
                        }
                        up = doc.parent(a);
                    }
                    false
                };
                let roots: Vec<u32> = ids
                    .iter()
                    .copied()
                    .filter(|&id| !inside(self, id))
                    .collect();
                roots
                    .into_iter()
                    .try_for_each(|id| run(self, Op::Remove { id }))
            }
            Command::MoveNodes { mut ids, delta } => {
                ids.sort_unstable();
                ids.dedup();
                for id in ids {
                    let node = self.node(id).ok_or(CommandError::MissingNode(id))?;
                    let (origin, rotation) = each_node!(node, n => (n.origin, n.rotation));
                    let origin = [origin[0] + delta[0], origin[1] + delta[1]];
                    run(
                        self,
                        Op::Transform {
                            id,
                            origin,
                            rotation,
                        },
                    )?;
                }
                Ok(())
            }
            Command::TransformNodes { nodes } => nodes.into_iter().try_for_each(|t| {
                let (id, origin, rotation) = (t.id, t.origin, t.rotation);
                run(
                    self,
                    Op::Transform {
                        id,
                        origin,
                        rotation,
                    },
                )
            }),
            Command::PlaceNode { id, parent, index } => {
                let node = self.node(id).ok_or(CommandError::MissingNode(id))?;
                self.check_placement(parent, matches!(node, Node::Section(_)))?;
                let index = index as usize;
                run(self, Op::Place { id, parent, index })
            }
            Command::SetProperty { id, property } => {
                property.check()?;
                run(self, Op::Set { id, property })
            }
            Command::SetChart { property } => {
                if let ChartProperty::Settings(s) = &property
                    && !s.seat_size.is_finite()
                {
                    return Err(CommandError::InvalidValue("seatSize"));
                }
                run(self, Op::Chart(property))
            }
            Command::AddCategory { index, category } => {
                let mut list = self.categories.clone();
                if list.len() >= MAX_CATEGORIES as usize {
                    return Err(CommandError::Limit("categories"));
                }
                if list.iter().any(|c| c.key == category.key) {
                    return Err(CommandError::DuplicateCategory(category.key));
                }
                if index as usize > list.len() {
                    return Err(CommandError::IndexOutOfRange);
                }
                list.insert(index as usize, category);
                run(self, Op::Categories(list))
            }
            Command::UpdateCategory { key, category } => {
                let mut list = self.categories.clone();
                let i = list.iter().position(|c| c.key == key);
                let i = i.ok_or_else(|| CommandError::MissingCategory(key.clone()))?;
                if category.key != key {
                    if list.iter().any(|c| c.key == category.key) {
                        return Err(CommandError::DuplicateCategory(category.key));
                    }
                    self.reassign_category(&key, Some(&category.key), &mut run)?;
                }
                if let Some(c) = list.get_mut(i) {
                    *c = category;
                }
                run(self, Op::Categories(list))
            }
            Command::RemoveCategory { key, replacement } => {
                let mut list = self.categories.clone();
                let i = list.iter().position(|c| c.key == key);
                let i = i.ok_or_else(|| CommandError::MissingCategory(key.clone()))?;
                if let Some(r) = &replacement
                    && (r == &key || !list.iter().any(|c| &c.key == r))
                {
                    return Err(CommandError::MissingCategory(r.clone()));
                }
                self.reassign_category(&key, replacement.as_deref(), &mut run)?;
                list.remove(i);
                run(self, Op::Categories(list))
            }
            Command::AddFloor { index, mut floor } => {
                let mut list = self.floors.clone();
                if list.len() >= MAX_FLOORS as usize {
                    return Err(CommandError::Limit("floors"));
                }
                if index as usize > list.len() {
                    return Err(CommandError::IndexOutOfRange);
                }
                let top = list.iter().map(|f| f.id).max().unwrap_or(0);
                floor.id = top.checked_add(1).ok_or(CommandError::IdsExhausted)?;
                check_floor(&floor)?;
                list.insert(index as usize, floor);
                run(self, Op::Floors(list))
            }
            Command::UpdateFloor { floor } => {
                let mut list = self.floors.clone();
                check_floor(&floor)?;
                let id = floor.id;
                let slot = list.iter_mut().find(|f| f.id == id);
                *slot.ok_or(CommandError::MissingFloor(id))? = floor;
                run(self, Op::Floors(list))
            }
            Command::RemoveFloor { id, replacement } => {
                let mut list = self.floors.clone();
                let i = list.iter().position(|f| f.id == id);
                let i = i.ok_or(CommandError::MissingFloor(id))?;
                if let Some(r) = replacement
                    && (r == id || !list.iter().any(|f| f.id == r))
                {
                    return Err(CommandError::MissingFloor(r));
                }
                let on: Vec<u32> = self
                    .slots
                    .iter()
                    .filter(|(_, s)| each_node!(&s.node, n => n.floor) == Some(id))
                    .map(|(&id, _)| id)
                    .collect();
                for node in on {
                    let property = Property::Floor(replacement);
                    run(self, Op::Set { id: node, property })?;
                }
                list.remove(i);
                run(self, Op::Floors(list))
            }
            Command::Composite { commands } => commands
                .into_iter()
                .try_for_each(|c| self.execute(c, inverse, created)),
        }
    }

    /// Only top-level sections hold children; sections are top-level.
    fn check_placement(&self, parent: Option<u32>, section: bool) -> Result<()> {
        match parent {
            Some(p) if section || self.parent(p).is_some() => Err(CommandError::Placement),
            _ => Ok(()),
        }
    }

    fn raise_next_oid(
        &mut self,
        next: u32,
        mut run: impl FnMut(&mut Self, Op) -> Result<()>,
    ) -> Result<()> {
        if next > self.next_oid {
            run(self, Op::NextOid(next))?;
        }
        Ok(())
    }

    fn reassign_category(
        &mut self,
        from: &str,
        to: Option<&str>,
        run: &mut impl FnMut(&mut Self, Op) -> Result<()>,
    ) -> Result<()> {
        let mut ops = Vec::new();
        for (&id, slot) in &self.slots {
            if category(&slot.node).is_some_and(|c| c.as_deref() == Some(from)) {
                let property = Property::Category(to.map(String::from));
                ops.push(Op::Set { id, property });
            }
            let overrides = match &slot.node {
                Node::RowBlock(b) => &b.seat_overrides,
                Node::Row(r) => &r.seat_overrides,
                _ => continue,
            };
            for (&oid, over) in overrides {
                if over.category.as_deref() == Some(from) {
                    let mut value = over.clone();
                    value.category = to.map(String::from);
                    ops.push(Op::Override {
                        id,
                        oid,
                        value: Some(value),
                    });
                }
            }
        }
        ops.into_iter().try_for_each(|op| run(self, op))
    }
}

impl Property {
    /// Rejects values that would leave the document unreadable.
    fn check(&self) -> Result<()> {
        let finite = |v: f64, field| {
            v.is_finite()
                .then_some(())
                .ok_or(CommandError::InvalidValue(field))
        };
        match self {
            Self::Priority(v)
            | Self::SeatPitch(v)
            | Self::RowPitch(v)
            | Self::Stagger(v)
            | Self::Skew(v) => finite(*v, self.field()),
            Self::Curve(Curve::Arc { radius: v } | Curve::Bulge { amount: v }) => {
                finite(*v, "curve")
            }
            Self::Outline(Outline::Points(p)) => p
                .points
                .iter()
                .flatten()
                .try_for_each(|&v| finite(v, "outline")),
            Self::Decoration(node) if !reparses(node) => Err(CommandError::InvalidValue("node")),
            _ => Ok(()),
        }
    }

    pub(crate) fn field(&self) -> &'static str {
        match self {
            Self::Name(_) => "name",
            Self::Floor(_) => "floor",
            Self::Locked(_) => "locked",
            Self::Hidden(_) => "hidden",
            Self::Label(_) => "label",
            Self::Category(_) => "category",
            Self::Priority(_) => "priority",
            Self::Entrance(_) => "entrance",
            Self::Outline(_) => "outline",
            Self::SeatPitch(_) => "seatPitch",
            Self::RowPitch(_) => "rowPitch",
            Self::Curve(_) => "curve",
            Self::Align(_) => "align",
            Self::Stagger(_) => "stagger",
            Self::Skew(_) => "skew",
            Self::RowLabels(_) => "rowLabels",
            Self::SeatLabels(_) => "seatLabels",
            Self::Decoration(_) => "decoration",
        }
    }

    /// Puts the value into the node and returns the old one, or gives the
    /// property back when the node has no such field.
    pub(crate) fn swap(self, node: &mut Node) -> Result<Property, Property> {
        use Property as P;
        if let P::Decoration(mut new) = self {
            let same = std::mem::discriminant(node) == std::mem::discriminant(&*new);
            if !same
                || !matches!(
                    node,
                    Node::Shape(_) | Node::Text(_) | Node::Icon(_) | Node::Image(_)
                )
            {
                return Err(P::Decoration(new));
            }
            let id = node_id(node);
            each_node!(&mut *new, n => n.id = id);
            return Ok(P::Decoration(Box::new(replace(node, *new))));
        }
        Ok(match (node, self) {
            (n, P::Name(v)) => P::Name(each_node!(n, x => replace(&mut x.name, v))),
            (n, P::Floor(v)) => P::Floor(each_node!(n, x => replace(&mut x.floor, v))),
            (n, P::Locked(v)) => P::Locked(each_node!(n, x => replace(&mut x.locked, v))),
            (n, P::Hidden(v)) => P::Hidden(each_node!(n, x => replace(&mut x.hidden, v))),
            (n, P::Label(v)) => match label_mut(n) {
                Some(f) => P::Label(replace(f, v)),
                None => return Err(P::Label(v)),
            },
            (n, P::Category(v)) => match category_mut(n) {
                Some(f) => P::Category(replace(f, v)),
                None => return Err(P::Category(v)),
            },
            (Node::Section(s), P::Priority(v)) => P::Priority(replace(&mut s.priority, v)),
            (Node::Section(s), P::Entrance(v)) => P::Entrance(replace(&mut s.entrance, v)),
            (Node::Section(s), P::Outline(v)) => P::Outline(replace(&mut s.outline, v)),
            (Node::RowBlock(b), P::SeatPitch(v)) => P::SeatPitch(replace(&mut b.seat_pitch, v)),
            (Node::Row(r), P::SeatPitch(v)) => P::SeatPitch(replace(&mut r.seat_pitch, v)),
            (Node::RowBlock(b), P::RowPitch(v)) => P::RowPitch(replace(&mut b.row_pitch, v)),
            (Node::RowBlock(b), P::Curve(v)) => P::Curve(replace(&mut b.curve, v)),
            (Node::RowBlock(b), P::Align(v)) => P::Align(replace(&mut b.align, v)),
            (Node::RowBlock(b), P::Stagger(v)) => P::Stagger(replace(&mut b.stagger, v)),
            (Node::RowBlock(b), P::Skew(v)) => P::Skew(replace(&mut b.skew, v)),
            (Node::RowBlock(b), P::RowLabels(v)) => {
                P::RowLabels(Box::new(replace(&mut b.row_labels, *v)))
            }
            (Node::RowBlock(b), P::SeatLabels(v)) => {
                P::SeatLabels(Box::new(replace(&mut b.seat_labels, *v)))
            }
            (Node::Row(r), P::SeatLabels(v)) => {
                P::SeatLabels(Box::new(replace(&mut r.seat_labels, *v)))
            }
            (Node::Table(t), P::SeatLabels(v)) => {
                P::SeatLabels(Box::new(replace(&mut t.seat_labels, *v)))
            }
            (_, p) => return Err(p),
        })
    }
}

fn label_mut(node: &mut Node) -> Option<&mut Option<String>> {
    match node {
        Node::Section(n) => Some(&mut n.label),
        Node::Row(n) => Some(&mut n.label),
        Node::Table(n) => Some(&mut n.label),
        Node::Booth(n) => Some(&mut n.label),
        Node::Area(n) => Some(&mut n.label),
        _ => None,
    }
}

fn category_mut(node: &mut Node) -> Option<&mut Option<String>> {
    match node {
        Node::Section(n) => Some(&mut n.category),
        Node::RowBlock(n) => Some(&mut n.category),
        Node::Row(n) => Some(&mut n.category),
        Node::Table(n) => Some(&mut n.category),
        Node::Booth(n) => Some(&mut n.category),
        Node::Area(n) => Some(&mut n.category),
        _ => None,
    }
}

fn category(node: &Node) -> Option<&Option<String>> {
    match node {
        Node::Section(n) => Some(&n.category),
        Node::RowBlock(n) => Some(&n.category),
        Node::Row(n) => Some(&n.category),
        Node::Table(n) => Some(&n.category),
        Node::Booth(n) => Some(&n.category),
        Node::Area(n) => Some(&n.category),
        _ => None,
    }
}

fn check_floor(floor: &Floor) -> Result<()> {
    match floor.focal_point {
        Some(p) if !p.iter().all(|v| v.is_finite()) => {
            Err(CommandError::InvalidValue("focalPoint"))
        }
        _ => Ok(()),
    }
}

/// True if the node survives a trip through JSON unchanged: every number is
/// finite, so the saved document parses again.
fn reparses(node: &Node) -> bool {
    serde_json::to_value(node)
        .and_then(serde_json::from_value::<Node>)
        .is_ok_and(|back| back == *node)
}

/// Counts the ids a node needs and its bookable objects (see
/// [`own_objects`]), with its children, checking the per-row limit before
/// anything is allocated. Stops counting once over the chart limit.
fn census(node: &Node, ids: &mut u64, objects: &mut u64) -> Result<()> {
    if *objects > u64::from(MAX_OBJECTS) {
        return Err(CommandError::Limit("objects"));
    }
    let widest = match node {
        Node::RowBlock(b) => b.rows.iter().map(|r| r.seats).max().unwrap_or(0),
        Node::Row(r) => r.seats,
        Node::Table(t) => t.seats,
        _ => 0,
    };
    if widest > MAX_SEATS_PER_ROW {
        return Err(CommandError::Limit("seats per row"));
    }
    let own = own_objects(node);
    // One id for the node, one per block row, one per object.
    let block_rows = match node {
        Node::RowBlock(b) => b.rows.len() as u64,
        _ => 0,
    };
    *ids += 1 + block_rows + own;
    *objects += own;
    if let Node::Section(s) = node {
        for c in &s.children {
            census(c, ids, objects)?;
        }
    }
    if *objects > u64::from(MAX_OBJECTS) {
        return Err(CommandError::Limit("objects"));
    }
    Ok(())
}

/// Bookable objects a node holds itself, children aside, as 02 §14 counts
/// them: seat positions (removed ones included), a table booked whole,
/// booths and areas.
fn own_objects(node: &Node) -> u64 {
    match node {
        Node::RowBlock(b) => b.rows.iter().map(|r| u64::from(r.seats)).sum(),
        Node::Row(r) => r.seats.into(),
        Node::Table(t) => u64::from(t.seats) + u64::from(t.table_oid.is_some()),
        Node::Booth(_) | Node::Area(_) => 1,
        Node::Section(_) | Node::Shape(_) | Node::Text(_) | Node::Icon(_) | Node::Image(_) => 0,
    }
}

/// Gives the node fresh ids from `next` in document order: node id, then each
/// row's `rid` and its seats as one run. Overrides follow their seats; one
/// whose oid is not on a seat is dropped.
fn renumber(node: &mut Node, next: &mut u32) {
    let mut take = |n: u32| {
        let first = *next;
        *next = next.saturating_add(n);
        first
    };
    each_node!(&mut *node, n => n.id = take(1));
    match node {
        Node::Section(s) => s.children.iter_mut().for_each(|c| renumber(c, next)),
        Node::RowBlock(b) => {
            let old = std::mem::take(&mut b.seat_overrides);
            for r in &mut b.rows {
                r.rid = take(1);
                let first = take(r.seats);
                remap(&old, &r.oids, r.seats, first, &mut b.seat_overrides);
                r.oids = run_of(first, r.seats);
            }
        }
        Node::Row(r) => {
            let old = std::mem::take(&mut r.seat_overrides);
            let first = take(r.seats);
            remap(&old, &r.oids, r.seats, first, &mut r.seat_overrides);
            r.oids = run_of(first, r.seats);
        }
        Node::Table(t) => {
            t.oids = run_of(take(t.seats), t.seats);
            if t.table_oid.is_some() {
                t.table_oid = Some(take(1));
            }
        }
        Node::Booth(b) => b.oid = take(1),
        Node::Area(a) => a.oid = take(1),
        Node::Shape(_) | Node::Text(_) | Node::Icon(_) | Node::Image(_) => {}
    }
}

fn run_of(first: u32, seats: u32) -> Vec<[u32; 2]> {
    if seats == 0 {
        Vec::new()
    } else {
        vec![[first, seats]]
    }
}

/// Moves the overrides of a row's seats (old `runs`) to the seats' new oids.
fn remap(
    old: &BTreeMap<u32, SeatOverride>,
    runs: &[[u32; 2]],
    seats: u32,
    first: u32,
    out: &mut BTreeMap<u32, SeatOverride>,
) {
    let mut pos = 0u32;
    for &[start, count] in runs {
        for (&oid, over) in old.range(start..start.saturating_add(count)) {
            let p = pos.saturating_add(oid - start);
            if p < seats {
                out.entry(first.saturating_add(p))
                    .or_insert_with(|| over.clone());
            }
        }
        pos = pos.saturating_add(count);
    }
}
