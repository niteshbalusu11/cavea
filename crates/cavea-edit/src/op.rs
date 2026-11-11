//! The edit operations every command is made of, and their inverses.
//!
//! An [`Op`] is a small, absolute change: applying one checks that it fits the
//! document, changes it, and returns the op that restores the previous state
//! exactly. Commands are translated into ops (`command.rs`); the undo and redo
//! stacks hold ops (`editor.rs`). Ops are internal: they carry ids, so they
//! are never taken from outside, where they could hand out an id twice.

use std::collections::BTreeMap;
use std::mem::{Discriminant, discriminant, replace};

use cavea_compiler::document::{BlockRow, Category, Floor, Node, SeatOverride};

use crate::command::{ChartProperty, CommandError, Property};
use crate::document::{Document, each_node, node_id};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(crate) enum Op {
    /// Inserts a subtree, ids included, at `index` in `parent`'s children.
    Insert {
        parent: Option<u32>,
        index: usize,
        node: Box<Node>,
    },
    Remove {
        id: u32,
    },
    /// Moves a node to `index` in `parent`'s children, counted after it has
    /// left its current place.
    Place {
        id: u32,
        parent: Option<u32>,
        index: usize,
    },
    Transform {
        id: u32,
        origin: [f64; 2],
        rotation: f64,
    },
    Set {
        id: u32,
        property: Property,
    },
    Chart(ChartProperty),
    Categories(Vec<Category>),
    Floors(Vec<Floor>),
    NextOid(u32),
    /// Replaces a row block's rows: adding and removing rows, in one step
    /// whatever their number.
    Rows {
        id: u32,
        rows: Vec<BlockRow>,
    },
    /// Replaces the row with the same `rid`.
    SetRow {
        id: u32,
        row: BlockRow,
    },
    Override {
        id: u32,
        oid: u32,
        value: Option<SeatOverride>,
    },
    /// Replaces a free-form row's removed positions.
    Removed {
        id: u32,
        removed: Vec<u32>,
    },
    /// Replaces a node's own data with a value of the same type and id; a
    /// section's children stay.
    Replace {
        id: u32,
        node: Box<Node>,
    },
}

/// What an op restores, for coalescing a gesture: of two inverses with the
/// same key, the earlier one alone restores the state before both.
/// Structural ops have no key and are always kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Transform(u32),
    Set(u32, Discriminant<Property>),
    Chart(Discriminant<ChartProperty>),
    Categories,
    Floors,
    NextOid,
    Row(u32, u32),
    Override(u32, u32),
    Removed(u32),
}

impl Op {
    pub(crate) fn key(&self) -> Option<Key> {
        Some(match self {
            Op::Insert { .. } | Op::Remove { .. } | Op::Place { .. } => return None,
            // Kept always: a row's own inverse (`SetRow`) may be recorded
            // after it, so undo must replay every row list in turn.
            Op::Rows { .. } => return None,
            // Kept always: it covers what every other key of the node does.
            Op::Replace { .. } => return None,
            Op::Removed { id, .. } => Key::Removed(*id),
            Op::Transform { id, .. } => Key::Transform(*id),
            Op::Set { id, property } => Key::Set(*id, discriminant(property)),
            Op::Chart(p) => Key::Chart(discriminant(p)),
            Op::Categories(_) => Key::Categories,
            Op::Floors(_) => Key::Floors,
            Op::NextOid(_) => Key::NextOid,
            Op::SetRow { id, row } => Key::Row(*id, row.rid),
            Op::Override { id, oid, .. } => Key::Override(*id, *oid),
        })
    }
}

/// A refused op, given back with the reason. Boxed: failure is rare.
type Failed = Box<(Op, CommandError)>;

impl Document {
    /// Applies `op` and returns its inverse, or gives the op back with the
    /// reason it does not fit, having changed nothing.
    pub(crate) fn apply_op(&mut self, op: Op) -> Result<Op, Failed> {
        self.revision += 1;
        let rev = self.revision;
        match op {
            Op::Insert {
                parent,
                index,
                node,
            } => {
                if let Err(e) = self.check_insert(parent, index, &node) {
                    return fail(
                        Op::Insert {
                            parent,
                            index,
                            node,
                        },
                        e,
                    );
                }
                let id = node_id(&node);
                // Checked above: the ids are free and the parent exists.
                let _ = self.insert_tree(parent, *node);
                self.child_list(parent).insert(index, id);
                self.touch_parent(parent);
                Ok(Op::Remove { id })
            }
            Op::Remove { id } => {
                let Some((parent, index)) = self.position(id) else {
                    return fail(Op::Remove { id }, CommandError::MissingNode(id));
                };
                self.child_list(parent).remove(index);
                self.touch_parent(parent);
                match self.take_tree(id) {
                    Some(node) => Ok(Op::Insert {
                        parent,
                        index,
                        node: Box::new(node),
                    }),
                    None => fail(Op::Remove { id }, CommandError::Internal),
                }
            }
            Op::Place { id, parent, index } => {
                if let Err(e) = self.check_place(id, parent, index) {
                    return fail(Op::Place { id, parent, index }, e);
                }
                let Some((old_parent, old_index)) = self.position(id) else {
                    return fail(Op::Place { id, parent, index }, CommandError::Internal);
                };
                self.child_list(old_parent).remove(old_index);
                self.child_list(parent).insert(index, id);
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.parent = parent;
                    slot.stamp = rev;
                }
                self.touch_parent(old_parent);
                self.touch_parent(parent);
                Ok(Op::Place {
                    id,
                    parent: old_parent,
                    index: old_index,
                })
            }
            Op::Transform {
                id,
                origin,
                rotation,
            } => {
                let finite = origin.iter().all(|v| v.is_finite()) && rotation.is_finite();
                let Some(slot) = self.slots.get_mut(&id).filter(|_| finite) else {
                    let e = if finite {
                        CommandError::MissingNode(id)
                    } else {
                        CommandError::InvalidValue("transform")
                    };
                    return fail(
                        Op::Transform {
                            id,
                            origin,
                            rotation,
                        },
                        e,
                    );
                };
                slot.stamp = rev;
                let (origin, rotation) = each_node!(&mut slot.node, n => (
                    replace(&mut n.origin, origin),
                    replace(&mut n.rotation, rotation),
                ));
                Ok(Op::Transform {
                    id,
                    origin,
                    rotation,
                })
            }
            Op::Set { id, property } => {
                let Some(slot) = self.slots.get_mut(&id) else {
                    return fail(Op::Set { id, property }, CommandError::MissingNode(id));
                };
                match property.swap(&mut slot.node) {
                    Ok(property) => {
                        slot.stamp = rev;
                        slot.content = rev;
                        Ok(Op::Set { id, property })
                    }
                    Err(property) => {
                        let field = property.field();
                        fail(
                            Op::Set { id, property },
                            CommandError::NotApplicable { id, field },
                        )
                    }
                }
            }
            Op::Chart(p) => {
                self.chart_stamp = rev;
                Ok(Op::Chart(match p {
                    ChartProperty::Name(v) => ChartProperty::Name(replace(&mut self.name, v)),
                    ChartProperty::Settings(v) => {
                        ChartProperty::Settings(replace(&mut self.settings, v))
                    }
                }))
            }
            Op::Categories(v) => {
                self.chart_stamp = rev;
                Ok(Op::Categories(replace(&mut self.categories, v)))
            }
            Op::Floors(v) => {
                self.chart_stamp = rev;
                Ok(Op::Floors(replace(&mut self.floors, v)))
            }
            Op::NextOid(v) => {
                self.chart_stamp = rev;
                Ok(Op::NextOid(replace(&mut self.next_oid, v)))
            }
            Op::Rows { id, rows } => match self.block_rows(id) {
                Ok(list) => {
                    let old = replace(list, rows);
                    self.touch_content(id);
                    Ok(Op::Rows { id, rows: old })
                }
                Err(e) => fail(Op::Rows { id, rows }, e),
            },
            Op::SetRow { id, row } => {
                let found = self.block_rows(id).and_then(|rows| {
                    let i = row_index(rows, id, row.rid)?;
                    rows.get_mut(i).ok_or(CommandError::Internal)
                });
                match found {
                    Ok(slot) => {
                        let old = replace(slot, row);
                        self.touch_content(id);
                        Ok(Op::SetRow { id, row: old })
                    }
                    Err(e) => fail(Op::SetRow { id, row }, e),
                }
            }
            Op::Override { id, oid, value } => {
                let old = match self.overrides(id) {
                    Ok(map) => match value {
                        Some(v) => map.insert(oid, v),
                        None => map.remove(&oid),
                    },
                    Err(e) => return fail(Op::Override { id, oid, value }, e),
                };
                self.touch_content(id);
                Ok(Op::Override {
                    id,
                    oid,
                    value: old,
                })
            }
            Op::Removed { id, removed } => match self.slots.get_mut(&id).map(|s| &mut s.node) {
                Some(Node::Row(r)) => {
                    let old = replace(&mut r.removed, removed);
                    self.touch_content(id);
                    Ok(Op::Removed { id, removed: old })
                }
                _ => fail(Op::Removed { id, removed }, CommandError::NotARowBlock(id)),
            },
            Op::Replace { id, node } => {
                let fits = |old: &Node| {
                    discriminant(old) == discriminant(&*node)
                        && node_id(&node) == id
                        && !matches!(&*node, Node::Section(s) if !s.children.is_empty())
                };
                match self.slots.get_mut(&id).filter(|s| fits(&s.node)) {
                    Some(slot) => {
                        let old = replace(&mut slot.node, *node);
                        self.touch_content(id);
                        Ok(Op::Replace {
                            id,
                            node: Box::new(old),
                        })
                    }
                    None => fail(Op::Replace { id, node }, CommandError::Internal),
                }
            }
        }
    }

    /// Applies ops in order. On failure the ones applied are undone and the
    /// rest, the failed one first, are given back with the reason.
    pub(crate) fn apply_all(&mut self, ops: Vec<Op>) -> Result<Vec<Op>, (Vec<Op>, CommandError)> {
        let mut inverse = Vec::with_capacity(ops.len());
        let mut ops = ops.into_iter();
        while let Some(op) = ops.next() {
            match self.apply_op(op) {
                Ok(inv) => inverse.push(inv),
                Err(failed) => {
                    let (op, e) = *failed;
                    let mut back = self.unwind(inverse);
                    back.push(op);
                    back.extend(ops);
                    return Err((back, e));
                }
            }
        }
        Ok(inverse)
    }

    /// Undoes `inverse` (inverses in the order their ops were applied) and
    /// returns the ops that redo it, in order.
    pub(crate) fn unwind(&mut self, inverse: Vec<Op>) -> Vec<Op> {
        let mut redo: Vec<Op> = inverse
            .into_iter()
            .rev()
            .filter_map(|op| self.apply_op(op).ok())
            .collect();
        redo.reverse();
        redo
    }

    fn check_insert(
        &self,
        parent: Option<u32>,
        index: usize,
        node: &Node,
    ) -> Result<(), CommandError> {
        if let Some(p) = parent {
            match self.slots.get(&p) {
                Some(slot) if matches!(slot.node, Node::Section(_)) => {}
                Some(_) => return Err(CommandError::NotASection(p)),
                None => return Err(CommandError::MissingNode(p)),
            }
        }
        if index > self.children(parent).len() {
            return Err(CommandError::IndexOutOfRange);
        }
        let mut ids = Vec::new();
        subtree_ids(node, &mut ids);
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        match ids.iter().find(|id| self.slots.contains_key(id)) {
            Some(&id) => Err(CommandError::DuplicateId(id)),
            None if ids.len() != before => Err(CommandError::Internal),
            None => Ok(()),
        }
    }

    fn check_place(&self, id: u32, parent: Option<u32>, index: usize) -> Result<(), CommandError> {
        let slot = self.slots.get(&id).ok_or(CommandError::MissingNode(id))?;
        let mut len = self.children(parent).len();
        if slot.parent == parent {
            len = len.saturating_sub(1);
        }
        if let Some(p) = parent {
            match self.slots.get(&p) {
                Some(s) if matches!(s.node, Node::Section(_)) => {}
                Some(_) => return Err(CommandError::NotASection(p)),
                None => return Err(CommandError::MissingNode(p)),
            }
            let mut up = Some(p);
            while let Some(a) = up {
                if a == id {
                    return Err(CommandError::Cycle { id, parent: p });
                }
                up = self.parent(a);
            }
        }
        if index > len {
            return Err(CommandError::IndexOutOfRange);
        }
        Ok(())
    }

    /// The node's parent and its index there.
    fn position(&self, id: u32) -> Option<(Option<u32>, usize)> {
        let parent = self.slots.get(&id)?.parent;
        let index = self.children(parent).iter().position(|&c| c == id)?;
        Some((parent, index))
    }

    fn child_list(&mut self, parent: Option<u32>) -> &mut Vec<u32> {
        match parent.and_then(|p| self.slots.get_mut(&p)) {
            Some(slot) => &mut slot.children,
            None => &mut self.roots,
        }
    }

    fn touch_parent(&mut self, parent: Option<u32>) {
        let rev = self.revision;
        match parent.and_then(|p| self.slots.get_mut(&p)) {
            Some(slot) => slot.stamp = rev,
            None => self.roots_stamp = rev,
        }
    }

    fn touch_content(&mut self, id: u32) {
        let rev = self.revision;
        if let Some(slot) = self.slots.get_mut(&id) {
            slot.stamp = rev;
            slot.content = rev;
        }
    }

    fn block_rows(&mut self, id: u32) -> Result<&mut Vec<BlockRow>, CommandError> {
        match self.slots.get_mut(&id).map(|s| &mut s.node) {
            Some(Node::RowBlock(b)) => Ok(&mut b.rows),
            Some(_) => Err(CommandError::NotARowBlock(id)),
            None => Err(CommandError::MissingNode(id)),
        }
    }

    fn overrides(&mut self, id: u32) -> Result<&mut BTreeMap<u32, SeatOverride>, CommandError> {
        match self.slots.get_mut(&id).map(|s| &mut s.node) {
            Some(Node::RowBlock(b)) => Ok(&mut b.seat_overrides),
            Some(Node::Row(r)) => Ok(&mut r.seat_overrides),
            Some(_) => Err(CommandError::NotARowBlock(id)),
            None => Err(CommandError::MissingNode(id)),
        }
    }
}

fn fail(op: Op, e: CommandError) -> Result<Op, Failed> {
    Err(Box::new((op, e)))
}

pub(crate) fn row_index(rows: &[BlockRow], id: u32, rid: u32) -> Result<usize, CommandError> {
    rows.iter()
        .position(|r| r.rid == rid)
        .ok_or(CommandError::MissingRow { id, rid })
}

fn subtree_ids(node: &Node, out: &mut Vec<u32>) {
    out.push(node_id(node));
    if let Node::Section(s) = node {
        for c in &s.children {
            subtree_ids(c, out);
        }
    }
}
