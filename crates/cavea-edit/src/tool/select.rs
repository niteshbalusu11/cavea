//! The select tool (`docs/07-designer.md` §5.1): click, shift-click, box
//! selection, entering and leaving levels, dragging and nudging nodes, and
//! the rotate, pivot and resize handles.

use std::collections::BTreeSet;
use std::mem::take;

use super::draw::{Live, budget};
use super::handles::{self, HandleKind, Step};
use super::{
    Button, Context, Cursor, Input, Key, Modifiers, Overlay, Pointer, Readout, Tool, tuning,
};
use cavea_compiler::document::Node;

use crate::command::{Command, NodeTransform};
use crate::document::each_node;
use crate::index::{P, Place, Rect, Select};
use crate::selection::{self, Level, Selection, View, pickable};

#[derive(Debug, Default)]
pub(crate) struct SelectTool {
    state: State,
    /// The last click's release, time and screen position: a press soon
    /// after and near it is a double click.
    last_click: Option<(f64, P)>,
    /// The pointer is over a selected item (hover, nothing pressed).
    over_selected: bool,
    /// A pivot the user moved, for the selection it was moved for.
    pivot: Option<(Level, BTreeSet<u32>, P)>,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Idle,
    /// Pressed, not yet dragged. `target` is the item pressed and whether it
    /// was selected before; `before` is the selection before the press.
    Pressed {
        down: Pointer,
        target: Option<(u32, bool)>,
        before: Selection,
    },
    /// Drawing a selection box from `start`, selecting on top of `base`: the
    /// items selected before the press, with Shift and at the same level.
    /// `before` is the selection before the press.
    Marquee {
        start: P,
        end: P,
        touch: bool,
        base: BTreeSet<u32>,
        before: Selection,
    },
    /// Dragging nodes inside a gesture: each node's origin and rotation at
    /// the start, and the section they are in.
    Moving {
        start: P,
        nodes: Vec<(u32, P, f64)>,
        parent: Option<u32>,
    },
    /// Pressed on a handle: a drag takes the handle, a release without one
    /// is an ordinary click there.
    Handle { down: Pointer, kind: HandleKind },
    /// Dragging the pivot; the moved pivot before the press.
    Pivot {
        before: Option<(Level, BTreeSet<u32>, P)>,
    },
    /// Rotating inside a gesture about `pivot` (world): each node's origin
    /// and rotation at the start, their section, the pointer's start angle
    /// and the angle turned so far, in degrees.
    Rotating {
        pivot: P,
        start: f64,
        nodes: Vec<(u32, P, f64)>,
        parent: Option<u32>,
        angle: f64,
    },
    /// Resizing one node with a handle: the node and its place at the start,
    /// the press in its own space, and the seats the chart may still take.
    Resizing {
        id: u32,
        kind: HandleKind,
        base: Box<Node>,
        place: Place,
        start: P,
        room: u64,
        readout: Option<Readout>,
        live: Live,
    },
    /// A press already acted on (a double click) or abandoned (a second
    /// press): nothing happens until it is released.
    Swallow,
}

impl Tool for SelectTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(p) => self.down(cx, p),
            Input::PointerMove(p) => self.drag(cx, p),
            Input::PointerUp(p) => self.up(cx, p),
            Input::PointerCancel => self.reset(cx),
            Input::KeyDown(key, mods) => self.key(cx, key, mods),
            Input::KeyUp(..) => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        self.last_click = None;
        match take(&mut self.state) {
            State::Moving { .. } | State::Rotating { .. } => {
                let _ = cx.editor.cancel_gesture();
            }
            State::Resizing { mut live, .. } => live.cancel(cx),
            State::Pivot { before } => self.pivot = before,
            State::Pressed { before, .. } | State::Marquee { before, .. } => {
                *cx.selection = before;
                cx.selection.reconcile(cx.editor.document());
            }
            State::Idle | State::Handle { .. } | State::Swallow => {}
        }
    }

    fn cursor(&self) -> Cursor {
        match self.state {
            State::Moving { .. } => Cursor::Move,
            State::Marquee { .. } => Cursor::Crosshair,
            State::Idle if self.over_selected => Cursor::Move,
            _ => Cursor::Default,
        }
    }

    fn overlay(&self, view: &View<'_>, sel: &Selection, out: &mut Overlay) {
        match &self.state {
            State::Marquee { start, end, .. } => out.marquee = Some(Rect::new(*start, *end)),
            State::Rotating { angle, .. } => out.readout = Some(Readout::Rotate { angle: *angle }),
            State::Resizing { readout, .. } => out.readout = *readout,
            _ => {}
        }
        out.handles = handles::handles(view, sel, self.pivot_for(sel));
    }
}

impl SelectTool {
    /// The pivot the user moved, if it was moved for this selection.
    fn pivot_for(&self, sel: &Selection) -> Option<P> {
        self.pivot
            .as_ref()
            .filter(|(level, items, _)| *level == sel.level() && items == sel.items())
            .map(|p| p.2)
    }

    /// The handle under a press, if any.
    fn handle_at(&self, cx: &mut Context<'_>, p: &Pointer) -> Option<HandleKind> {
        let (view, sel) = cx.split(p.tolerance());
        let found = handles::handles(&view, sel, self.pivot_for(sel));
        handles::hit(&found, p).map(|h| h.kind)
    }

    /// Starts the gesture of the handle pressed at `p`.
    fn start_handle(&mut self, cx: &mut Context<'_>, p: Pointer, kind: HandleKind) -> State {
        let room = budget(cx);
        let (view, sel) = cx.split(p.tolerance());
        let pivot = self.pivot_for(sel);
        let Some((ids, parent)) = handles::targets(&view, sel) else {
            return State::Swallow;
        };
        let doc = view.doc;
        match kind {
            HandleKind::Pivot => State::Pivot {
                before: self.pivot.clone(),
            },
            HandleKind::Rotate => {
                let pivot = pivot
                    .or_else(|| handles::frame(&view, &ids).map(|q| handles::centre(&q)))
                    .unwrap_or(p.world);
                let nodes = ids
                    .iter()
                    .filter_map(|&id| {
                        let node = doc.node(id)?;
                        Some(each_node!(node, n => (id, n.origin, n.rotation)))
                    })
                    .collect();
                if cx.editor.begin_gesture().is_err() {
                    return State::Swallow;
                }
                let start = (p.world[1] - pivot[1]).atan2(p.world[0] - pivot[0]);
                State::Rotating {
                    pivot,
                    start: start.to_degrees(),
                    nodes,
                    parent,
                    angle: 0.0,
                }
            }
            kind => {
                let [id] = ids[..] else {
                    return State::Swallow;
                };
                let (Some(base), Some(place)) = (doc.node(id), view.index.place(id)) else {
                    return State::Swallow;
                };
                State::Resizing {
                    id,
                    kind,
                    base: Box::new(base.clone()),
                    place,
                    start: place.invert(p.world),
                    room,
                    readout: None,
                    live: Live::default(),
                }
            }
        }
    }

    fn down(&mut self, cx: &mut Context<'_>, p: Pointer) {
        if !matches!(self.state, State::Idle) {
            // A second press during a drag abandons it (04 §14).
            self.reset(cx);
            self.state = State::Swallow;
            return;
        }
        if p.button != Button::Primary {
            return;
        }
        if let Some(kind) = self.handle_at(cx, &p) {
            self.state = State::Handle { down: p, kind };
            return;
        }
        self.press(cx, p);
    }

    /// A press that is not on a handle.
    fn press(&mut self, cx: &mut Context<'_>, p: Pointer) {
        let double = self.last_click.take().is_some_and(|(time, at)| {
            let near = (p.screen[0] - at[0]).hypot(p.screen[1] - at[1]);
            (0.0..=tuning::DOUBLE_CLICK_MS).contains(&(p.time - time))
                && near <= tuning::DOUBLE_CLICK_PX
        });
        let (view, sel) = cx.split(p.tolerance());
        let before = sel.clone();
        let (level, item) = locate(&view, sel.level(), p.world);
        if level != sel.level() {
            sel.set(view.doc, level, []);
        }
        if double {
            if let Some(id) = item {
                selection::enter(&view, sel, id, Some(p.world));
            }
            self.state = State::Swallow;
            return;
        }
        let target = item.map(|id| {
            let was = sel.items().contains(&id);
            if !was {
                if !p.mods.shift {
                    sel.items_mut().clear();
                }
                sel.items_mut().insert(id);
            }
            (id, was)
        });
        self.state = State::Pressed {
            down: p,
            target,
            before,
        };
    }

    fn drag(&mut self, cx: &mut Context<'_>, p: Pointer) {
        match &mut self.state {
            State::Idle => {
                let (view, sel) = cx.split(p.tolerance());
                let over = view.hit(sel.level(), p.world);
                self.over_selected = over.is_some_and(|i| sel.items().contains(&i));
            }
            State::Pressed {
                down,
                target,
                before,
            } => {
                if !p.dragged_from(down) {
                    return;
                }
                let (down, target, before) = (*down, *target, take(before));
                let parent = match cx.selection.level() {
                    Level::Top => Some(None),
                    Level::Section(s) => Some(Some(s)),
                    // Rows and seats are not moved yet: a drag selects them.
                    Level::Rows(_) | Level::Seats(_) => None,
                };
                self.state = match (target, parent) {
                    (Some(_), Some(parent)) => start_move(cx, down.world, parent),
                    _ => {
                        let same = before.level() == cx.selection.level();
                        let base = match down.mods.shift && same {
                            true => before.items().clone(),
                            false => BTreeSet::new(),
                        };
                        State::Marquee {
                            start: down.world,
                            end: down.world,
                            touch: false,
                            base,
                            before,
                        }
                    }
                };
                self.drag(cx, p);
            }
            State::Marquee {
                start,
                end,
                touch,
                base,
                ..
            } => {
                (*end, *touch) = (p.world, p.mods.alt);
                let mode = if *touch {
                    Select::Touch
                } else {
                    Select::Contain
                };
                let (view, sel) = cx.split(0.0);
                let hits = view.select(sel.level(), Rect::new(*start, *end), mode);
                *sel.items_mut() = base.iter().copied().chain(hits).collect();
            }
            State::Moving {
                start,
                nodes,
                parent,
            } => {
                let delta = snap_translation([p.world[0] - start[0], p.world[1] - start[1]]);
                let v = cx.index.to_parent(*parent, delta);
                let nodes = nodes
                    .iter()
                    .map(|&(id, o, rotation)| NodeTransform {
                        id,
                        origin: [o[0] + v[0], o[1] + v[1]],
                        rotation,
                    })
                    .collect();
                // A step that would leave the finite range is skipped.
                let _ = cx.editor.apply(Command::TransformNodes { nodes });
            }
            State::Handle { down, kind } => {
                if p.dragged_from(down) {
                    let (down, kind) = (*down, *kind);
                    self.state = self.start_handle(cx, down, kind);
                    self.drag(cx, p);
                }
            }
            State::Pivot { .. } => {
                let sel = &cx.selection;
                self.pivot = Some((sel.level(), sel.items().clone(), p.world));
            }
            State::Rotating {
                pivot,
                start,
                nodes,
                parent,
                angle,
            } => {
                let now = (p.world[1] - pivot[1]).atan2(p.world[0] - pivot[0]);
                let mut a = crate::draft::normalise(now.to_degrees() - *start);
                if p.mods.shift {
                    a = (a / 15.0).round() * 15.0;
                }
                *angle = snap_angle(a);
                let centre = cx.index.to_local(*parent, *pivot);
                let command = handles::rotate(nodes, centre, *angle);
                let _ = cx.editor.apply(command);
            }
            State::Resizing {
                id,
                kind,
                base,
                place,
                start,
                room,
                readout,
                live,
            } => {
                let q = place.invert(p.world);
                let d = [q[0] - start[0], q[1] - start[1]];
                match handles::resize(base, *id, *kind, d, q, p.mods, *room) {
                    Step::Base => {
                        live.clear(cx);
                        *readout = None;
                    }
                    Step::Show(command, shown) => {
                        if live.show(cx, command) {
                            *readout = Some(shown);
                        }
                    }
                    Step::Keep => {}
                }
            }
            State::Swallow => {}
        }
    }

    fn up(&mut self, cx: &mut Context<'_>, p: Pointer) {
        // The release is the drag's last position.
        if !matches!(self.state, State::Idle | State::Swallow) {
            self.drag(cx, p);
        }
        match take(&mut self.state) {
            State::Pressed { down, target, .. } => {
                let items = cx.selection.items_mut();
                match target {
                    Some((id, true)) if down.mods.shift => drop(items.remove(&id)),
                    Some((id, true)) => *items = BTreeSet::from([id]),
                    Some((_, false)) => {}
                    None if down.mods.shift => {}
                    None => items.clear(),
                }
                self.last_click = Some((p.time, p.screen));
            }
            State::Moving { start, .. } => {
                let _ = cx.editor.end_gesture();
                // A moved pivot moves with its selection.
                let delta = snap_translation([p.world[0] - start[0], p.world[1] - start[1]]);
                if let Some((_, _, at)) = &mut self.pivot {
                    *at = [at[0] + delta[0], at[1] + delta[1]];
                }
            }
            State::Rotating { .. } => {
                let _ = cx.editor.end_gesture();
            }
            State::Resizing { mut live, .. } => drop(live.commit(cx)),
            // Not dragged: an ordinary click.
            State::Handle { down, .. } => {
                self.press(cx, down);
                self.up(cx, p);
            }
            State::Idle | State::Marquee { .. } | State::Pivot { .. } | State::Swallow => {}
        }
    }

    fn key(&mut self, cx: &mut Context<'_>, key: Key, mods: Modifiers) {
        let idle = matches!(self.state, State::Idle);
        let step = if mods.shift { 10.0 } else { 1.0 };
        match key {
            Key::Escape if !idle => self.reset(cx),
            Key::Escape => {
                selection::exit(cx.editor.document(), cx.index, cx.selection);
            }
            _ if !idle => {}
            Key::Enter => drop(step_in(cx)),
            Key::Char('a') if mods.ctrl => select_all(cx),
            Key::ArrowLeft => nudge(cx, [-step, 0.0]),
            Key::ArrowRight => nudge(cx, [step, 0.0]),
            Key::ArrowUp => nudge(cx, [0.0, -step]),
            Key::ArrowDown => nudge(cx, [0.0, step]),
            Key::Char(_) | Key::Backspace => {}
        }
    }
}

/// The level a press at `point` acts on and the item it hits there. A press
/// that hits nothing at the current level stays there if it is on the node
/// holding the level (a gap between seats, a section's empty part);
/// otherwise it is tried at the level holding that node, out to the top.
fn locate(view: &View<'_>, mut level: Level, point: P) -> (Level, Option<u32>) {
    loop {
        if let Some(item) = view.hit(level, point) {
            return (level, Some(item));
        }
        let holder = match level {
            Level::Top => return (level, None),
            Level::Section(id) | Level::Rows(id) | Level::Seats(id) => id,
        };
        if view.index.on_node(view.doc, holder, point, view.tol) {
            return (level, None);
        }
        level = match view.doc.parent(holder) {
            Some(s) => Level::Section(s),
            None => Level::Top,
        };
    }
}

/// Where snapping (DSN-030) will adjust a proposed move.
fn snap_translation(delta: P) -> P {
    delta
}

/// Where snapping (DSN-030) will adjust a proposed rotation, in degrees.
fn snap_angle(angle: f64) -> f64 {
    angle
}

/// Opens a gesture for dragging the selected nodes that can be picked.
fn start_move(cx: &mut Context<'_>, start: P, parent: Option<u32>) -> State {
    let doc = cx.editor.document();
    let nodes: Vec<(u32, P, f64)> = cx
        .selection
        .items()
        .iter()
        .filter(|&&id| pickable(doc, cx.floor, id))
        .filter_map(|&id| {
            let node = doc.node(id)?;
            Some(each_node!(node, n => (id, n.origin, n.rotation)))
        })
        .collect();
    if nodes.is_empty() || cx.editor.begin_gesture().is_err() {
        return State::Swallow;
    }
    State::Moving {
        start,
        nodes,
        parent,
    }
}

/// Moves the selected nodes that can be picked by `v` on screen, as one undo
/// entry per call.
pub(crate) fn nudge(cx: &mut Context<'_>, v: P) {
    let parent = match cx.selection.level() {
        Level::Top => None,
        Level::Section(s) => Some(s),
        Level::Rows(_) | Level::Seats(_) => return,
    };
    let doc = cx.editor.document();
    let ids: Vec<u32> = cx
        .selection
        .items()
        .iter()
        .copied()
        .filter(|&id| pickable(doc, cx.floor, id))
        .collect();
    if !ids.is_empty() {
        let delta = cx.index.to_parent(parent, v);
        let _ = cx.editor.apply(Command::MoveNodes { ids, delta });
    }
}

/// Selects every item at the current level that can be picked.
pub(crate) fn select_all(cx: &mut Context<'_>) {
    let (view, sel) = cx.split(0.0);
    let all = view.all(sel.level());
    sel.set(view.doc, sel.level(), all);
}

/// Steps into the one selected item, selecting everything inside it.
pub(crate) fn step_in(cx: &mut Context<'_>) -> bool {
    let (view, sel) = cx.split(0.0);
    let [id] = sel.items().iter().copied().collect::<Vec<_>>()[..] else {
        return false;
    };
    if !view.pickable(sel.level(), id) || !selection::enter(&view, sel, id, None) {
        return false;
    }
    let all = view.all(sel.level());
    sel.set(view.doc, sel.level(), all);
    true
}
