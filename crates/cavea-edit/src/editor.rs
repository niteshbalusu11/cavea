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
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    gesture: Option<Gesture>,
    /// Raised by every change of the stacks or the gesture state.
    generation: u64,
}

/// An undo or redo entry: the ops that restore the state before it, and a
/// stable key naming what it did, for the shell to translate (07 §9).
#[derive(Debug)]
struct Entry {
    label: &'static str,
    ops: Vec<Op>,
}

#[derive(Debug, Default)]
struct Gesture {
    /// The key of the entry it becomes: the first label it was given.
    label: Option<&'static str>,
    /// Inverses in the order their ops were applied.
    inverse: Vec<Op>,
    /// Membership only; never iterated.
    keys: HashSet<Key>,
    /// The id counter and high-water mark at the start.
    ids: (u64, u32),
}

/// What an applied command did that its sender cannot know in advance.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    /// Ids of the nodes made by `createNode`, in order.
    pub created: Vec<u32>,
}

impl Editor {
    pub fn load(document: ChartDocument) -> Result<Self, LoadError> {
        Ok(Self::from_document(Document::load(document)?))
    }

    /// An editor of a document loaded already, with empty history.
    pub fn from_document(doc: Document) -> Self {
        Self {
            doc,
            undo: VecDeque::new(),
            redo: Vec::new(),
            gesture: None,
            generation: 0,
        }
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Applies a command completely, or returns why not and changes nothing.
    /// Outside a gesture it is one undo entry and clears redo.
    pub fn apply(&mut self, command: Command) -> Result<Applied, CommandError> {
        self.apply_as(command, None)
    }

    /// [`Self::apply`], naming the undo entry `label` rather than by the
    /// command ([`Command::label`]). Inside a gesture the first label wins.
    pub fn apply_as(
        &mut self,
        command: Command,
        label: Option<&'static str>,
    ) -> Result<Applied, CommandError> {
        let label = label.unwrap_or_else(|| command.label());
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
                g.label.get_or_insert(label);
                for op in inverse {
                    if op.key().is_none_or(|k| g.keys.insert(k)) {
                        g.inverse.push(op);
                    }
                }
            }
            None => self.push(label, inverse),
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
        self.generation += 1;
        Ok(())
    }

    /// Closes the gesture as one undo entry (none if it changed nothing).
    pub fn end_gesture(&mut self) -> Result<(), CommandError> {
        let g = self.gesture.take().ok_or(CommandError::NoGesture)?;
        self.generation += 1;
        self.push(g.label.unwrap_or("gesture"), g.inverse);
        Ok(())
    }

    /// Restores the state at the start of the gesture; leaves no entry, and
    /// redo as it was. Ids it took are not handed out again.
    pub fn cancel_gesture(&mut self) -> Result<(), CommandError> {
        let g = self.gesture.take().ok_or(CommandError::NoGesture)?;
        self.generation += 1;
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
        let Some(Entry { label, ops }) = self.undo.pop_back() else {
            return Ok(false);
        };
        self.generation += 1;
        match self.doc.apply_all(ops) {
            Ok(mut ops) => {
                ops.reverse();
                self.redo.push(Entry { label, ops });
                Ok(true)
            }
            Err((ops, e)) => {
                self.undo.push_back(Entry { label, ops });
                Err(e)
            }
        }
    }

    /// Redoes the last undone entry; false if there is none.
    pub fn redo(&mut self) -> Result<bool, CommandError> {
        if self.gesture.is_some() {
            return Err(CommandError::GestureOpen);
        }
        let Some(Entry { label, ops }) = self.redo.pop() else {
            return Ok(false);
        };
        self.generation += 1;
        match self.doc.apply_all(ops) {
            Ok(mut ops) => {
                ops.reverse();
                self.undo.push_back(Entry { label, ops });
                Ok(true)
            }
            Err((ops, e)) => {
                self.redo.push(Entry { label, ops });
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

    /// The keys of the entries undo and redo would apply next.
    pub fn labels(&self) -> (Option<&'static str>, Option<&'static str>) {
        let undo = self.undo.back().map(|e| e.label);
        (undo, self.redo.last().map(|e| e.label))
    }

    /// Raised by every change of the undo and redo stacks or of the gesture
    /// state: what the history view shows cannot change without it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Pushes the inverses of a step (in application order) as one entry.
    fn push(&mut self, label: &'static str, mut ops: Vec<Op>) {
        if ops.is_empty() {
            return;
        }
        ops.reverse();
        self.generation += 1;
        self.redo.clear();
        self.undo.push_back(Entry { label, ops });
        if self.undo.len() > MAX_UNDO {
            self.undo.pop_front();
        }
    }

    /// Serialised size of each undo entry, oldest first.
    #[cfg(test)]
    pub(crate) fn entry_sizes(&self) -> Vec<usize> {
        self.undo
            .iter()
            .map(|e| serde_json::to_vec(&e.ops).map_or(0, |v| v.len()))
            .collect()
    }
}
