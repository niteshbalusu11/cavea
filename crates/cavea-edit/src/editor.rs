//! The editor: the document with its undo and redo stacks and gestures
//! (`docs/07-designer.md` §9).
//!
//! An undo entry is the list of ops that restores the state before it, so
//! undoing never copies the document: it applies those ops, which return the
//! ops that redo it. Entries hold only what their commands changed: a deleted
//! block's data, a moved node's old transform.
//!
//! A gesture (a drag, a scrub of a number field) opens with
//! [`Editor::begin_gesture`]; each command applied while it is open changes
//! the document at once, and [`Editor::end_gesture`] closes it as one entry.
//! Within a gesture the first inverse recorded for a node's transform, a
//! property, a row or a seat override is kept and later ones are dropped,
//! since the first alone restores the state at the start; so a drag of any
//! length holds one old transform per node. [`Editor::cancel_gesture`]
//! restores the start and leaves no entry; the ids the gesture took stay
//! taken. A tool showing a drawing restarts its own gesture instead
//! (`rewind_gesture`), which keeps the same ids for the node being drawn.

use std::collections::{HashSet, VecDeque};

use cavea_compiler::ChartDocument;

use crate::command::{Command, CommandError};
use crate::document::{Document, LoadError};
use crate::op::{Key, Op};

/// Undo entries kept (07 §9).
pub const MAX_UNDO: usize = 500;

#[derive(Debug)]
pub struct Editor {
    doc: Document,
    undo: VecDeque<Vec<Op>>,
    redo: Vec<Vec<Op>>,
    gesture: Option<Gesture>,
}

#[derive(Debug, Default)]
struct Gesture {
    /// Inverses in the order their ops were applied.
    inverse: Vec<Op>,
    /// Membership only; never iterated.
    keys: HashSet<Key>,
    /// The id counter and high-water mark at the start.
    ids: (u64, u32),
}

/// What an applied command did that its sender cannot know in advance.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Ids of the nodes made by `createNode`, in order.
    pub created: Vec<u32>,
}

impl Editor {
    pub fn load(document: ChartDocument) -> Result<Self, LoadError> {
        Ok(Self {
            doc: Document::load(document)?,
            undo: VecDeque::new(),
            redo: Vec::new(),
            gesture: None,
        })
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Applies a command completely, or returns why not and changes nothing.
    /// Outside a gesture it is one undo entry and clears redo.
    pub fn apply(&mut self, command: Command) -> Result<Applied, CommandError> {
        let ids = (self.doc.fresh, self.doc.high_water);
        let mut inverse = Vec::new();
        let mut created = Vec::new();
        if let Err(e) = self.doc.execute(command, &mut inverse, &mut created) {
            self.doc.unwind(inverse);
            // Nothing holds the ids taken, so they may be handed out again.
            (self.doc.fresh, self.doc.high_water) = ids;
            return Err(e);
        }
        match &mut self.gesture {
            Some(g) => {
                for op in inverse {
                    if op.key().is_none_or(|k| g.keys.insert(k)) {
                        g.inverse.push(op);
                    }
                }
            }
            None => self.push(inverse),
        }
        Ok(Applied { created })
    }

    pub fn begin_gesture(&mut self) -> Result<(), CommandError> {
        if self.gesture.is_some() {
            return Err(CommandError::GestureOpen);
        }
        self.gesture = Some(Gesture {
            ids: (self.doc.fresh, self.doc.high_water),
            ..Gesture::default()
        });
        Ok(())
    }

    /// Closes the gesture as one undo entry (none if it changed nothing).
    pub fn end_gesture(&mut self) -> Result<(), CommandError> {
        let g = self.gesture.take().ok_or(CommandError::NoGesture)?;
        self.push(g.inverse);
        Ok(())
    }

    /// Restores the state at the start of the gesture; leaves no entry, and
    /// redo as it was. Ids it took are not handed out again.
    pub fn cancel_gesture(&mut self) -> Result<(), CommandError> {
        let g = self.gesture.take().ok_or(CommandError::NoGesture)?;
        self.doc.unwind(g.inverse);
        Ok(())
    }

    /// Restores the state at the start of the open gesture, the id counter
    /// included, and keeps it open: what a tool's preview is replaced with
    /// gets the ids the old preview had. Only for the tools, whose previews
    /// no one else holds an id of; outside, only `cancel_gesture`.
    pub(crate) fn rewind_gesture(&mut self) -> Result<(), CommandError> {
        let g = self.gesture.as_mut().ok_or(CommandError::NoGesture)?;
        let inverse = std::mem::take(&mut g.inverse);
        g.keys.clear();
        let ids = g.ids;
        self.doc.unwind(inverse);
        (self.doc.fresh, self.doc.high_water) = ids;
        Ok(())
    }

    pub fn gesture_open(&self) -> bool {
        self.gesture.is_some()
    }

    /// Undoes the last entry; false if there is none.
    pub fn undo(&mut self) -> Result<bool, CommandError> {
        if self.gesture.is_some() {
            return Err(CommandError::GestureOpen);
        }
        let Some(entry) = self.undo.pop_back() else {
            return Ok(false);
        };
        match self.doc.apply_all(entry) {
            Ok(mut redo) => {
                redo.reverse();
                self.redo.push(redo);
                Ok(true)
            }
            Err((entry, e)) => {
                self.undo.push_back(entry);
                Err(e)
            }
        }
    }

    /// Redoes the last undone entry; false if there is none.
    pub fn redo(&mut self) -> Result<bool, CommandError> {
        if self.gesture.is_some() {
            return Err(CommandError::GestureOpen);
        }
        let Some(entry) = self.redo.pop() else {
            return Ok(false);
        };
        match self.doc.apply_all(entry) {
            Ok(mut undo) => {
                undo.reverse();
                self.undo.push_back(undo);
                Ok(true)
            }
            Err((entry, e)) => {
                self.redo.push(entry);
                Err(e)
            }
        }
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Pushes the inverses of a step (in application order) as one entry.
    fn push(&mut self, mut inverse: Vec<Op>) {
        if inverse.is_empty() {
            return;
        }
        inverse.reverse();
        self.redo.clear();
        self.undo.push_back(inverse);
        if self.undo.len() > MAX_UNDO {
            self.undo.pop_front();
        }
    }

    /// Serialised size of each undo entry, oldest first.
    #[cfg(test)]
    pub(crate) fn entry_sizes(&self) -> Vec<usize> {
        self.undo
            .iter()
            .map(|e| serde_json::to_vec(e).map_or(0, |v| v.len()))
            .collect()
    }
}
