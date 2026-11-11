//! What the drawing tools share (`docs/07-designer.md` §4): where a new node
//! goes, the gesture that shows it while it is drawn, and numeric entry.

use cavea_core::limits::MAX_OBJECTS;

use super::{Context, Key, Pointer};
use crate::command::Command;
use crate::draft::Draft;
use crate::index::P;
use crate::selection::{Level, Selection};

/// Where a new node goes: its parent, its place among the parent's
/// children (the end, at the start of the gesture) and its floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spot {
    pub(crate) parent: Option<u32>,
    pub(crate) index: u32,
    pub(crate) floor: Option<u32>,
}

impl Spot {
    /// Into the section whose children are selected (the user stepped into
    /// it) if the press is on it (07 §5.1) or it is empty; otherwise the top
    /// level, on the floor shown.
    pub(crate) fn at(cx: &Context<'_>, p: &Pointer) -> Spot {
        let doc = cx.editor.document();
        let parent = match cx.selection.level() {
            Level::Section(s)
                if doc.children(Some(s)).is_empty()
                    || cx.index.on_node(doc, s, p.world, p.tolerance()) =>
            {
                Some(s)
            }
            _ => None,
        };
        Spot::new(cx.editor.document(), parent, cx.floor)
    }

    /// A node at the end of `parent`'s children; at the top level it names
    /// the floor shown unless that is the first floor (where a node without
    /// one is).
    pub(crate) fn new(doc: &crate::Document, parent: Option<u32>, floor: Option<u32>) -> Spot {
        let first = doc.floors().first().map(|f| f.id);
        let floor = match parent {
            None if floor != first => floor,
            _ => None,
        };
        let index = u32::try_from(doc.children(parent).len()).unwrap_or(u32::MAX);
        Spot {
            parent,
            index,
            floor,
        }
    }

    /// A world point in the parent's space, where drafts are drawn.
    pub(crate) fn local(&self, cx: &Context<'_>, p: P) -> P {
        cx.index.to_local(self.parent, p)
    }

    pub(crate) fn level(&self) -> Level {
        self.parent.map_or(Level::Top, Level::Section)
    }

    /// The command creating `draft` here, if the draft is valid.
    pub(crate) fn create(&self, category: Option<&str>, draft: &Draft) -> Option<Command> {
        let node = draft.node(category, self.floor).ok()?;
        Some(Command::CreateNode {
            parent: self.parent,
            index: self.index,
            node: Box::new(node),
        })
    }
}

/// A change shown live: a gesture holding the result of one command on the
/// state at the gesture's start. Showing another command cancels the
/// gesture (which hands back its ids) and applies the new one in a fresh
/// one, so the document always holds exactly one command's result, with the
/// same ids each time, whatever path the pointer took.
#[derive(Debug, Default)]
pub(crate) struct Live {
    shown: Option<Box<Command>>,
    open: bool,
    created: Option<u32>,
}

impl Live {
    /// Shows `command`'s result. If it is refused, the last result shown
    /// stays and false is returned.
    pub(crate) fn show(&mut self, cx: &mut Context<'_>, command: Command) -> bool {
        if self.shown.as_deref() == Some(&command) {
            return true;
        }
        let last = self.shown.take();
        self.hide(cx);
        if cx.editor.begin_gesture().is_err() {
            return false;
        }
        self.open = true;
        if let Ok(applied) = cx.editor.apply(command.clone()) {
            self.created = applied.created.first().copied();
            self.shown = Some(Box::new(command));
            return true;
        }
        if let Some(last) = last
            && let Ok(applied) = cx.editor.apply((*last).clone())
        {
            self.created = applied.created.first().copied();
            self.shown = Some(last);
        }
        false
    }

    /// Back to the state at the start, the gesture still to come.
    pub(crate) fn hide(&mut self, cx: &mut Context<'_>) {
        if self.open {
            let _ = cx.editor.cancel_gesture();
        }
        *self = Live::default();
    }

    /// Closes the gesture as one undo entry; the node created, if any.
    pub(crate) fn commit(&mut self, cx: &mut Context<'_>) -> Option<u32> {
        let created = self.created;
        if self.open {
            let _ = cx.editor.end_gesture();
        }
        *self = Live::default();
        created
    }
}

/// Commits a drawing and selects the node it made, at its level.
pub(crate) fn finish(cx: &mut Context<'_>, live: &mut Live, spot: &Spot) {
    if let Some(id) = live.commit(cx) {
        select(cx.selection, cx.editor.document(), spot.level(), id);
    }
}

pub(crate) fn select(sel: &mut Selection, doc: &crate::Document, level: Level, id: u32) {
    sel.set(doc, level, [id]);
}

/// Bookable objects the chart may still take (02 §14).
pub(crate) fn budget(cx: &Context<'_>) -> u64 {
    u64::from(MAX_OBJECTS).saturating_sub(cx.editor.document().objects())
}

pub(crate) fn dist(a: P, b: P) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// `to`, turned about `from` to the nearest multiple of 15° (Shift).
pub(crate) fn snap15(from: P, to: P) -> P {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let step = 15f64.to_radians();
    let angle = (dy.atan2(dx) / step).round() * step;
    let r = dx.hypot(dy);
    [from[0] + r * angle.cos(), from[1] + r * angle.sin()]
}

/// Numbers typed during a drag (07 §4): fields separated by `x`, each
/// digits with at most one point; an empty field keeps the dragged value.
/// At most 32 characters.
#[derive(Debug, Default, Clone)]
pub(crate) struct Entry(pub(crate) String);

impl Entry {
    /// Takes a digit, point, `x` or Backspace; false for any other key.
    pub(crate) fn key(&mut self, key: Key) -> bool {
        match key {
            Key::Backspace => drop(self.0.pop()),
            Key::Char(c @ ('0'..='9' | '.' | 'x')) if self.0.len() < 32 => self.0.push(c),
            Key::Char('0'..='9' | '.' | 'x') => {}
            _ => return false,
        }
        true
    }

    /// The fields typed, in order: `None` for one left empty or unreadable.
    pub(crate) fn fields(&self) -> [Option<f64>; 3] {
        let mut out = [None; 3];
        for (slot, text) in out.iter_mut().zip(self.0.split('x')) {
            *slot = text.parse::<f64>().ok().filter(|v| v.is_finite());
        }
        out
    }
}

/// A count from a typed number: whole, at least 1, at most `max`.
pub(crate) fn count(v: f64, max: u32) -> u32 {
    v.round().clamp(1.0, f64::from(max.max(1))) as u32
}
