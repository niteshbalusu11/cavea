//! The session's calls for the shell's chrome (`docs/07-designer.md` §2,
//! §3, §14): which views changed, and the views and calls of the top bar,
//! the status bar, the inspector and the layers panel.

use cavea_compiler::document::Floor;

use super::Session;
use crate::command::Property;
use crate::command::{Command, CommandError};
use crate::editor::Applied;
use crate::index::Rect;
use crate::inspector::{self, Field, InspectorView};
use crate::layers::{self, Drop, LayerClick, LayersPage};
use crate::selection::{Level, shown};
use crate::views::{
    self, FloorsView, HistoryView, Mark, SelectionView, StatusView, VIEWS, ViewName,
};

use super::ToolView;

/// Bytes of a floor's name.
const MAX_NAME: usize = 256;

impl Session {
    /// The views that may differ from when this was last called (all of
    /// them the first time), in [`VIEWS`] order; the rest are as they were.
    /// The shell calls it once per animation frame and reads only these:
    /// that is how view models are "throttled to one per frame" (07 §14),
    /// since the edit core has no clock. It costs a comparison of the
    /// selection with the last one seen and a few numbers per view, and
    /// builds no view.
    pub fn changed(&mut self) -> Vec<ViewName> {
        let (now, then) = (&self.selection, &self.watch.selection);
        if now.level() != then.level() || now.items() != then.items() {
            self.watch.selection = now.clone();
            self.watch.selections += 1;
        }
        let mut out = Vec::new();
        for (i, &view) in VIEWS.iter().enumerate() {
            let mark = self.mark(view);
            if let Some(seen) = self.watch.seen.0.get_mut(i)
                && *seen != Some(mark)
            {
                *seen = Some(mark);
                out.push(view);
            }
        }
        out
    }

    /// Numbers that change whenever the view may: the revisions and counts
    /// it is built from.
    fn mark(&self, view: ViewName) -> Mark {
        let doc = self.editor.document();
        let w = &self.watch;
        let floor = self.floor.map_or(0, |f| u64::from(f) + 1);
        let (rev, structure) = (doc.revision(), doc.structure_revision());
        let pieces = self.validator.pieces;
        match view {
            ViewName::History => [self.editor.generation(), 0, 0, 0, 0, 0, 0, 0],
            ViewName::Title => [doc.chart_stamp, 0, 0, 0, 0, 0, 0, 0],
            ViewName::Floors => [structure, floor, 0, 0, 0, 0, 0, 0],
            ViewName::Status => [rev, 0, 0, 0, 0, 0, 0, 0],
            // `fit` depends on the floor shown.
            ViewName::Selection => [rev, w.selections, floor, 0, 0, 0, 0, 0],
            ViewName::Inspector => [rev, w.selections, 0, 0, 0, 0, 0, 0],
            ViewName::Layers => [
                structure,
                w.selections,
                self.layers.generation,
                floor,
                0,
                0,
                0,
                0,
            ],
            ViewName::Categories => [structure, w.tools, 0, 0, 0, 0, 0, 0],
            ViewName::Numbering => {
                let shown = u64::from(self.preview.is_some());
                [rev, w.selections, w.previews, shown, floor, 0, 0, 0]
            }
            ViewName::Validation => [rev, pieces, 0, 0, 0, 0, 0, 0],
            ViewName::Overlay => [
                rev,
                w.selections,
                floor,
                w.guides,
                pieces,
                w.inputs,
                w.tools,
                0,
            ],
            ViewName::Cursor => [self.cursor() as u64, 0, 0, 0, 0, 0, 0, 0],
            ViewName::Tool => [w.tools, 0, 0, 0, 0, 0, 0, 0],
            ViewName::Guides => [w.guides, structure, floor, 0, 0, 0, 0, 0],
        }
    }

    /// Undo and redo for the top bar.
    pub fn history(&self) -> HistoryView {
        let (undo, redo) = self.editor.labels();
        HistoryView {
            undo,
            redo,
            gesture: self.editor.gesture_open(),
        }
    }

    /// The chart's name, for the top bar.
    pub fn title(&self) -> &str {
        self.document().name()
    }

    pub fn status(&self) -> StatusView {
        views::status(self.document())
    }

    /// The selection summary; its `bounds` is the box "zoom to selection"
    /// frames.
    pub fn selection_view(&self) -> SelectionView {
        let sel = &self.selection;
        let items: Vec<u32> = sel.items().iter().copied().collect();
        let (kinds, locked, hidden) = views::kinds(self.document(), sel.level(), &items);
        let outlines = self.view().outlines(sel);
        SelectionView {
            level: sel.level(),
            count: u32::try_from(items.len()).unwrap_or(u32::MAX),
            ids: items.iter().copied().take(views::SELECTION_IDS).collect(),
            kinds,
            locked,
            hidden,
            bounds: Rect::around(outlines.iter().flatten().copied()),
            within: match sel.level() {
                Level::Section(id) => self.document().node(id).and_then(crate::layers::name),
                _ => None,
            },
        }
    }

    /// The world box "zoom to fit" frames: every node drawn on the floor
    /// shown, hidden ones excepted.
    pub fn fit(&self) -> Option<Rect> {
        let doc = self.document();
        let roots = doc.children(None).iter().copied();
        let drawn = roots.filter(|&id| shown(doc, self.floor, id));
        drawn
            .filter_map(|id| self.index.bounds(id))
            .fold(None, |a, b| Rect::union(a, Some(b)))
    }

    pub fn tool_view(&self) -> ToolView {
        ToolView {
            tool: self.tool_id,
            held: self.held,
            category: self.category.clone(),
            snap: self.snap.settings,
        }
    }

    /// The floor switcher's list and the floor shown.
    pub fn floors(&self) -> FloorsView {
        views::floors(self.document(), self.floor)
    }

    /// Adds a floor at the end of the list (07 §5.6); returns its id.
    pub fn add_floor(&mut self, name: &str, level: i32) -> Result<u32, CommandError> {
        if name.len() > MAX_NAME {
            return Err(CommandError::InvalidValue("name"));
        }
        let index = u32::try_from(self.document().floors().len()).unwrap_or(u32::MAX);
        let floor = Floor {
            id: 0,
            name: name.to_owned(),
            level,
            focal_point: None,
        };
        self.apply(Command::AddFloor { index, floor })?;
        let added = self.document().floors().get(index as usize).map(|f| f.id);
        added.ok_or(CommandError::Internal)
    }

    pub fn rename_floor(&mut self, id: u32, name: &str) -> Result<Applied, CommandError> {
        if name.len() > MAX_NAME {
            return Err(CommandError::InvalidValue("name"));
        }
        let floors = self.document().floors();
        let mut floor = floors.iter().find(|f| f.id == id).cloned();
        let floor = floor.as_mut().ok_or(CommandError::MissingFloor(id))?;
        floor.name = name.to_owned();
        let floor = floor.clone();
        self.apply(Command::UpdateFloor { floor })
    }

    /// Deletes a floor; its nodes move to `replacement`, or to the first
    /// floor when it is `null`. Floors cannot be reordered yet: there is no
    /// command for it (DSN-053).
    pub fn delete_floor(
        &mut self,
        id: u32,
        replacement: Option<u32>,
    ) -> Result<Applied, CommandError> {
        self.apply(Command::RemoveFloor { id, replacement })
    }

    /// The inspector's fields for the selection, or the chart's settings.
    pub fn inspector(&self) -> InspectorView {
        inspector::view(self.document(), &self.selection)
    }

    /// Sets one inspector field on every selected node (or the chart), as
    /// one undo entry named by the field; refused with a typed error and
    /// nothing changed when a value is out of its limits or a node lacks
    /// the field. While a scrub is open, each call replaces the last one's
    /// effect inside it.
    pub fn set_field(&mut self, field: &Field) -> Result<Applied, CommandError> {
        let Some(last) = self.scrub.take() else {
            self.reset_tool();
            let targets = inspector::targets(&self.selection);
            let command = inspector::command(self.document(), &targets, field)?;
            let result = self.editor.apply_as(command, Some(field.key()));
            self.settle();
            return result;
        };
        // The scrub stays open whatever happens: rewound to its start, the
        // new value shown, or else the last one again.
        let result = field.check().and_then(|()| {
            self.editor.rewind_gesture()?;
            let targets = inspector::targets(&self.selection);
            let command = inspector::command(self.document(), &targets, field)?;
            let applied = self.editor.apply_as(command.clone(), Some(field.key()))?;
            Ok((command, applied))
        });
        let (shown, result) = match result {
            Ok((command, applied)) => (Some(command), Ok(applied)),
            Err(e) => {
                if self.editor.gesture_open() && last.is_some() {
                    let _ = self.editor.rewind_gesture();
                    if let Some(c) = last.clone() {
                        let _ = self.editor.apply_as(c, Some(field.key()));
                    }
                }
                (last, Err(e))
            }
        };
        self.scrub = Some(shown);
        self.settle();
        result
    }

    /// Opens a scrub (a number field dragged, 07 §9): the [`Self::set_field`]
    /// calls until [`Self::end_scrub`] are one undo entry, and
    /// [`Self::cancel_scrub`] (Escape) restores the start. Anything else
    /// called on the session ends it first, as a release would.
    pub fn begin_scrub(&mut self) -> Result<(), CommandError> {
        self.reset_tool();
        self.editor.begin_gesture()?;
        self.scrub = Some(None);
        Ok(())
    }

    /// Closes the scrub as one undo entry (none if nothing changed).
    pub fn end_scrub(&mut self) {
        if self.scrub.take().is_some() {
            let _ = self.editor.end_gesture();
            self.settle();
        }
    }

    pub fn cancel_scrub(&mut self) {
        if self.scrub.take().is_some() {
            let _ = self.editor.cancel_gesture();
            self.settle();
        }
    }

    /// `count` rows of the layers tree from row `start`, with the total.
    pub fn layers(&mut self, start: u32, count: u32) -> LayersPage {
        let doc = self.editor.document();
        self.layers
            .page(doc, self.floor, &self.selection, start, count)
    }

    /// Shows or hides a section's children in the tree.
    pub fn expand_layer(&mut self, id: u32, open: bool) {
        self.layers.expand(id, open);
    }

    /// Expands the section holding `id` and returns its row, for scrolling
    /// to it when the canvas selects it; `None` if it is not in the tree of
    /// the floor shown.
    pub fn reveal_layer(&mut self, id: u32) -> Option<u32> {
        let doc = self.editor.document();
        self.layers.reveal(doc, self.floor, id)
    }

    /// Selects from the tree: row `id` alone, toggled (Ctrl), or the range
    /// from the row last clicked (Shift) among its siblings; locked and
    /// hidden nodes included (07 §5.1). False for an unknown node.
    pub fn click_layer(&mut self, id: u32, how: LayerClick) -> bool {
        let doc = self.editor.document();
        let picked = self.layers.click(doc, self.floor, &self.selection, id, how);
        match picked {
            Some((level, items)) => {
                self.select(level, items);
                true
            }
            None => false,
        }
    }

    /// Moves nodes `ids` to `drop`, in document order, as one undo entry:
    /// reordered among siblings or moved between sections and the top
    /// level, keeping every seat where it is on the canvas (`placeNode`)
    /// and a node leaving its section on the floor it was shown on. Refused
    /// as the structure rule (07 §2.1) asks: a section into a section
    /// (`Placement`), into a node that is not a section (`NotASection`), a
    /// section into itself (`Cycle`), a drop beside a node being moved
    /// (`InvalidValue("target")`). Locked nodes move too: the panel is
    /// explicit. The moved nodes are selected.
    pub fn move_layers(&mut self, ids: &[u32], drop: Drop) -> Result<Applied, CommandError> {
        self.reset_tool();
        let command = layers::place(self.document(), ids, drop)?;
        let result = self.editor.apply_as(command, Some("moveLayers"));
        self.settle();
        if result.is_ok() {
            let doc = self.editor.document();
            let parent = ids.first().and_then(|&id| doc.parent(id));
            let level = parent.map_or(Level::Top, Level::Section);
            self.selection.set(doc, level, ids.iter().copied());
        }
        result
    }

    /// Locks or unlocks one row of the tree, whatever is selected.
    pub fn lock_layer(&mut self, id: u32, locked: bool) -> Result<Applied, CommandError> {
        let property = Property::Locked(locked);
        self.apply(Command::SetProperty { id, property })
    }

    /// Hides or shows one row of the tree, whatever is selected.
    pub fn hide_layer(&mut self, id: u32, hidden: bool) -> Result<Applied, CommandError> {
        let property = Property::Hidden(hidden);
        self.apply(Command::SetProperty { id, property })
    }
}
