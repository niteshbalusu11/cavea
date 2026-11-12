//! The text tool (`docs/07-designer.md` §4): click, type. The Rust tool owns
//! no text field: it asks the chrome for one (`Overlay::text`), the chrome
//! sends the whole string on every change (`Session::set_text`), and Enter or
//! a click elsewhere commits, Escape cancels. The select tool edits an
//! existing text node the same way after a double click.

use cavea_compiler::document::{Align, Node};

use super::draw::{Live, Spot, finish};
use super::snap;
use super::{Button, Context, Cursor, Input, Key, Overlay, TextField, Tool};
use crate::command::{Command, Property};
use crate::draft::{Draft, TEXT_COLOR, TEXT_SIZE, TEXT_WEIGHT};
use crate::index::P;
use crate::selection::{Level, Selection, View};

/// Text being typed: a new node at `at`, or an existing one. Its gesture
/// holds the text shown; an empty string shows nothing (a new node does
/// not exist yet, an existing one keeps its text).
#[derive(Debug)]
pub(crate) struct TextEdit {
    spot: Spot,
    /// The parent-space position of a new node.
    at: P,
    /// The node being edited, with its data before the edit.
    existing: Option<(u32, Box<Node>)>,
    text: String,
    live: Live,
}

impl TextEdit {
    pub(crate) fn new(cx: &Context<'_>, spot: Spot, world: P) -> Self {
        Self {
            spot,
            at: spot.local(cx, world),
            existing: None,
            text: String::new(),
            live: Live::default(),
        }
    }

    /// Edits the text node `id`, if it is one.
    pub(crate) fn edit(cx: &Context<'_>, id: u32, level: Level) -> Option<Self> {
        let doc = cx.editor.document();
        let node @ Node::Text(t) = doc.node(id)? else {
            return None;
        };
        let parent = match level {
            Level::Section(s) => Some(s),
            _ => None,
        };
        Some(Self {
            spot: Spot::new(doc, parent, cx.floor),
            at: t.origin,
            existing: Some((id, Box::new(node.clone()))),
            text: t.text.clone(),
            live: Live::default(),
        })
    }

    pub(crate) fn set(&mut self, cx: &mut Context<'_>, text: &str) {
        self.text = text.chars().take(1_024).collect();
        if self.text.is_empty() {
            return self.live.clear(cx);
        }
        let command = match &self.existing {
            Some((id, node)) => {
                let mut node = node.clone();
                if let Node::Text(t) = &mut *node {
                    t.text.clone_from(&self.text);
                }
                // Over 256 bytes is refused (E006): the last text stays.
                if !(1..=256).contains(&self.text.len()) {
                    return;
                }
                Some(Command::SetProperty {
                    id: *id,
                    property: Property::Decoration(node),
                })
            }
            None => self.spot.create(
                None,
                &Draft::Text {
                    at: self.at,
                    text: self.text.clone(),
                },
            ),
        };
        if let Some(command) = command {
            self.live.show(cx, command);
        }
    }

    pub(crate) fn commit(&mut self, cx: &mut Context<'_>) {
        if let Some((id, _)) = &self.existing {
            let id = *id;
            self.live.commit(cx);
            super::draw::select(cx.selection, cx.editor.document(), self.spot.level(), id);
        } else {
            finish(cx, &mut self.live, &self.spot);
        }
    }

    pub(crate) fn cancel(&mut self, cx: &mut Context<'_>) {
        self.live.cancel(cx);
    }

    pub(crate) fn field(&self, view: &View<'_>) -> TextField {
        let node = self.existing.as_ref().map(|e| e.0).or(self.live.created());
        let (at, turn) = match self.spot.parent.and_then(|s| view.index.place(s)) {
            Some(place) => (place.apply(self.at), place.angle()),
            None => (self.at, 0.0),
        };
        // The node as drawn now, else a new one's defaults (07 §4).
        let shown = node.and_then(|id| match view.doc.node(id) {
            Some(Node::Text(t)) => Some(t),
            _ => None,
        });
        let (size, rotation, align, weight, color) = match (shown, &self.existing) {
            (Some(t), _) => (t.size, t.rotation, t.align, t.weight, t.color.clone()),
            (None, Some((_, n))) => match &**n {
                Node::Text(t) => (t.size, t.rotation, t.align, t.weight, t.color.clone()),
                _ => defaults(),
            },
            (None, None) => defaults(),
        };
        TextField {
            node,
            at,
            text: self.text.clone(),
            size,
            angle: (turn + rotation).rem_euclid(360.0),
            align,
            weight,
            color,
        }
    }
}

/// A new text's size, rotation, alignment, weight and colour (07 §4).
fn defaults() -> (f64, f64, Align, u16, String) {
    (TEXT_SIZE, 0.0, Align::Center, TEXT_WEIGHT, TEXT_COLOR.to_owned())
}

#[derive(Debug, Default)]
pub(crate) struct TextTool {
    edit: Option<TextEdit>,
    /// A press already acted on: nothing happens until its release.
    swallow: bool,
}

impl Tool for TextTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(_) if self.swallow => {}
            // A click away commits the text being typed.
            Input::PointerDown(_) if self.edit.is_some() => {
                if let Some(mut e) = self.edit.take() {
                    e.commit(cx);
                }
                self.swallow = true;
            }
            Input::PointerDown(p) if p.button == Button::Primary => {
                let (view, sel) = cx.split(p.tolerance());
                let level = sel.level();
                let hit = view
                    .hit(level, p.world)
                    .filter(|&id| matches!(view.doc.node(id), Some(Node::Text(_))));
                self.edit = match hit {
                    Some(id) => TextEdit::edit(cx, id, level),
                    None => {
                        let spot = Spot::at(cx, &p);
                        let at = snap::point(cx, &p, None, spot.parent);
                        Some(TextEdit::new(cx, spot, at))
                    }
                };
                self.swallow = true;
            }
            Input::PointerUp(_) => self.swallow = false,
            Input::KeyDown(Key::Enter, _) => {
                if let Some(mut e) = self.edit.take() {
                    e.commit(cx);
                }
            }
            Input::PointerCancel | Input::KeyDown(Key::Escape, _) => self.reset(cx),
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        if let Some(mut e) = self.edit.take() {
            e.cancel(cx);
        }
        self.swallow = false;
    }

    fn cursor(&self) -> Cursor {
        Cursor::Crosshair
    }

    fn overlay(&self, view: &View<'_>, _: &Selection, out: &mut Overlay) {
        out.text = self.edit.as_ref().map(|e| e.field(view));
    }

    fn text(&mut self, cx: &mut Context<'_>, text: &str) {
        if let Some(e) = &mut self.edit {
            e.set(cx, text);
        }
    }
}
