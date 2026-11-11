//! A designer session (`docs/07-designer.md` §2.3): the editor, the spatial
//! index, the selection, the floor shown and the active tool. Everything
//! from outside, input or command, goes through it, so the index and the
//! selection are brought up to date after every change.

use cavea_compiler::ChartDocument;

use crate::command::Property;
use crate::command::{Command, CommandError};
use crate::document::{Document, LoadError};
use crate::draft::Draft;
use crate::editor::{Applied, Editor};
use crate::index::{Rect, SpatialIndex};
use crate::section;
use crate::selection::{self, Level, Selection};
use crate::tool::snap::{Snap, SnapSettings};
use crate::tool::{
    self, ArcTool, BlockTool, Context, Cursor, Input, Overlay, RowTool, SelectTool, ShapeKindTool,
    ShapeTool, TextTool, Tool, ToolId,
};
use cavea_compiler::document::{Node, Outline, Points};

#[derive(Debug)]
pub struct Session {
    editor: Editor,
    index: SpatialIndex,
    selection: Selection,
    floor: Option<u32>,
    /// The active category: new seats take it (07 §7).
    category: Option<String>,
    tool: Box<dyn Tool>,
    tool_id: ToolId,
    snap: Snap,
}

impl Session {
    pub fn load(document: ChartDocument) -> Result<Self, LoadError> {
        let editor = Editor::load(document)?;
        let floor = editor.document().floors().first().map(|f| f.id);
        let category = editor
            .document()
            .categories()
            .first()
            .map(|c| c.key.clone());
        let mut session = Self {
            editor,
            index: SpatialIndex::new(),
            selection: Selection::default(),
            floor,
            category,
            tool: Box::new(SelectTool::default()),
            tool_id: ToolId::Select,
            snap: Snap::default(),
        };
        session.settle();
        Ok(session)
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn document(&self) -> &Document {
        self.editor.document()
    }

    pub fn index(&self) -> &SpatialIndex {
        &self.index
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The floor shown: only its nodes are picked on the canvas.
    pub fn floor(&self) -> Option<u32> {
        self.floor
    }

    pub fn tool(&self) -> ToolId {
        self.tool_id
    }

    /// Switches the tool; the old one abandons what it was doing first, so
    /// no gesture is left open.
    pub fn set_tool(&mut self, id: ToolId) {
        self.with_tool(|tool, cx| tool.reset(cx));
        self.tool = match id {
            ToolId::Select => Box::new(SelectTool::default()),
            ToolId::RowBlock => Box::new(BlockTool::default()),
            ToolId::ArcBlock => Box::new(ArcTool::default()),
            ToolId::Row => Box::new(RowTool::default()),
            ToolId::Rect => Box::new(ShapeTool::new(ShapeKindTool::Rect)),
            ToolId::Ellipse => Box::new(ShapeTool::new(ShapeKindTool::Ellipse)),
            ToolId::Polygon => Box::new(ShapeTool::new(ShapeKindTool::Polygon)),
            ToolId::Line => Box::new(ShapeTool::new(ShapeKindTool::Line)),
            ToolId::Text => Box::new(TextTool::default()),
            ToolId::Section => Box::new(ShapeTool::new(ShapeKindTool::Section)),
        };
        self.tool_id = id;
    }

    /// The category new seats take; at load, the chart's first.
    pub fn category(&self) -> Option<&str> {
        self.category.as_deref()
    }

    pub fn set_category(&mut self, key: Option<String>) {
        self.category = key;
    }

    /// The whole string of the text field the tool asked for
    /// (`Overlay::text`), on every change.
    pub fn set_text(&mut self, text: &str) {
        self.with_tool(|tool, cx| tool.text(cx, text));
    }

    /// Creates the node a drawing tool would make from these numbers, at the
    /// end of `parent`'s children (positions in its space), as one undo
    /// entry, and selects it (07 §15: every tool's result without a
    /// pointer). At the top level it goes on the floor shown.
    pub fn insert(&mut self, parent: Option<u32>, draft: &Draft) -> Result<u32, CommandError> {
        self.reset_tool();
        let spot = tool::Spot::new(self.editor.document(), parent, self.floor);
        let node = draft.node(self.category.as_deref(), spot.floor)?;
        let result = self.editor.apply(Command::CreateNode {
            parent,
            index: spot.index,
            node: Box::new(node),
        });
        let id = result.and_then(|a| a.created.first().copied().ok_or(CommandError::Internal));
        self.settle();
        if let Ok(id) = id {
            self.selection
                .set(self.editor.document(), spot.level(), [id]);
        }
        id
    }

    /// Creates a top-level section around top-level nodes `ids`, which keep
    /// their place on the canvas, with an explicit `outline` (world points)
    /// or an automatic one, as one undo entry, and selects it (07 §5.5).
    pub fn group(
        &mut self,
        ids: &[u32],
        outline: Option<Vec<[f64; 2]>>,
    ) -> Result<u32, CommandError> {
        self.reset_tool();
        let spot = tool::Spot::new(self.editor.document(), None, self.floor);
        let style = (self.category.as_deref(), spot.floor);
        let id = section::group(&mut self.editor, &self.index, ids, outline, style);
        self.settle();
        if let Ok(id) = id {
            self.selection.set(self.editor.document(), Level::Top, [id]);
        }
        id
    }

    /// Dissolves a section: its children return to the top level in place,
    /// as one undo entry.
    pub fn dissolve(&mut self, section: u32) -> Result<(), CommandError> {
        self.reset_tool();
        let result = section::dissolve(&mut self.editor, section);
        self.settle();
        result
    }

    /// Replaces a section's automatic outline with explicit points equal to
    /// the outline the compiler computes now (07 §5.5), as one `setProperty`.
    pub fn convert_outline(&mut self, id: u32) -> Result<Applied, CommandError> {
        let auto = matches!(
            self.document().node(id),
            Some(Node::Section(s)) if s.outline == Outline::Auto
        );
        let points = section::outline(self.document(), &self.index, id)
            .filter(|p| auto && !p.is_empty())
            .ok_or(CommandError::InvalidValue("outline"))?;
        self.apply(Command::SetProperty {
            id,
            property: Property::Outline(Outline::Points(Points { points })),
        })
    }

    /// Passes input to the active tool. A pointer event that is not finite,
    /// or has a zoom of 0 or less, is dropped, except that such a release
    /// cancels: the drag it would have ended does not stay open.
    pub fn input(&mut self, mut input: Input) {
        match &input {
            Input::PointerUp(p) if !p.valid() => input = Input::PointerCancel,
            Input::PointerDown(p) | Input::PointerMove(p) if !p.valid() => return,
            _ => {}
        }
        self.snap.guides.clear();
        if matches!(input, Input::PointerDown(_)) {
            self.snap.release();
        }
        self.with_tool(|tool, cx| tool.input(cx, &input));
        if matches!(input, Input::PointerUp(_) | Input::PointerCancel) {
            self.snap.guides.clear();
        }
    }

    pub fn snap_settings(&self) -> SnapSettings {
        self.snap.settings
    }

    /// Turns each kind of snapping on or off, and sets the grid step (a
    /// step that is not finite and above 0 turns the grid off).
    pub fn set_snap(&mut self, settings: SnapSettings) {
        self.snap.settings = settings;
    }

    /// Applies a command from outside the canvas (the inspector, a panel).
    /// The active tool abandons what it was doing first.
    pub fn apply(&mut self, command: Command) -> Result<Applied, CommandError> {
        self.reset_tool();
        let result = self.editor.apply(command);
        self.settle();
        result
    }

    pub fn undo(&mut self) -> Result<bool, CommandError> {
        self.reset_tool();
        let result = self.editor.undo();
        self.settle();
        result
    }

    pub fn redo(&mut self) -> Result<bool, CommandError> {
        self.reset_tool();
        let result = self.editor.redo();
        self.settle();
        result
    }

    /// Selects from outside the canvas (the layers or validation panel):
    /// locked and hidden nodes included. Items not at `level` are dropped;
    /// a level that does not exist is the top level.
    pub fn select(&mut self, level: Level, items: impl IntoIterator<Item = u32>) {
        self.reset_tool();
        self.selection.set(self.editor.document(), level, items);
    }

    /// Selects every item at the current level that can be picked (Ctrl+A).
    pub fn select_all(&mut self) {
        self.reset_tool();
        self.with_tool(|_, cx| tool::select_all(cx));
    }

    /// Steps into the one selected item, selecting everything in it (Enter).
    pub fn step_in(&mut self) -> bool {
        self.reset_tool();
        let mut stepped = false;
        self.with_tool(|_, cx| stepped = tool::step_in(cx));
        stepped
    }

    /// Steps out one level, or clears the selection at the top (Escape).
    pub fn step_out(&mut self) {
        self.reset_tool();
        selection::exit(self.editor.document(), &self.index, &mut self.selection);
    }

    /// Shows another floor; the selection is cleared. False for a floor the
    /// chart does not list.
    pub fn set_floor(&mut self, floor: u32) -> bool {
        if !self.document().floors().iter().any(|f| f.id == floor) {
            return false;
        }
        self.reset_tool();
        self.floor = Some(floor);
        self.selection = Selection::default();
        true
    }

    pub fn cursor(&self) -> Cursor {
        self.tool.cursor()
    }

    /// What the canvas draws over the chart now.
    pub fn overlay(&self) -> Overlay {
        let view = selection::View {
            doc: self.editor.document(),
            index: &self.index,
            floor: self.floor,
            tol: 0.0,
        };
        let outlines = view.outlines(&self.selection);
        let frame = match outlines[..] {
            [one] => Some(one),
            _ => Rect::around(outlines.iter().flatten().copied()).map(|r| r.corners()),
        };
        let mut out = Overlay {
            outlines,
            frame,
            sections: self.section_outlines(),
            guides: self.snap.guides.clone(),
            ..Overlay::default()
        };
        self.tool.overlay(&view, &self.selection, &mut out);
        out
    }

    /// The outlines, in the world, of the selected sections and of the one
    /// whose children are selected.
    fn section_outlines(&self) -> Vec<Vec<[f64; 2]>> {
        let doc = self.document();
        let held = match self.selection.level() {
            Level::Section(s) => Some(s),
            _ => None,
        };
        let top = matches!(self.selection.level(), Level::Top);
        let ids = held.into_iter().chain(
            top.then(|| self.selection.items().iter().copied())
                .into_iter()
                .flatten(),
        );
        ids.filter_map(|id| {
            let points = match doc.node(id) {
                Some(Node::Section(s)) => match &s.outline {
                    Outline::Points(p) => p.points.clone(),
                    Outline::Auto => section::outline(doc, &self.index, id)?,
                },
                _ => return None,
            };
            points.iter().map(|&p| self.index.to_world(id, p)).collect()
        })
        .filter(|p: &Vec<_>| !p.is_empty())
        .collect()
    }

    fn reset_tool(&mut self) {
        self.snap.guides.clear();
        self.with_tool(|tool, cx| tool.reset(cx));
    }

    fn with_tool(&mut self, f: impl FnOnce(&mut dyn Tool, &mut Context<'_>)) {
        let mut cx = Context {
            editor: &mut self.editor,
            index: &self.index,
            selection: &mut self.selection,
            floor: self.floor,
            category: self.category.as_deref(),
            snap: &mut self.snap,
        };
        f(self.tool.as_mut(), &mut cx);
        self.settle();
    }

    /// Brings the index, the selection and the floor shown up to date.
    fn settle(&mut self) {
        let doc = self.editor.document();
        self.index.sync(doc);
        self.selection.reconcile(doc);
        let listed = |f| doc.floors().iter().any(|l| l.id == f);
        if !self.floor.is_some_and(listed) {
            let first = doc.floors().first().map(|f| f.id);
            if self.floor != first {
                self.floor = first;
                self.selection = Selection::default();
            }
        }
    }
}
