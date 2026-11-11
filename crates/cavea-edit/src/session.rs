//! A designer session (`docs/07-designer.md` §2.3): the editor, the spatial
//! index, the selection, the floor shown and the active tool. Everything
//! from outside, input or command, goes through it, so the index and the
//! selection are brought up to date after every change.

use cavea_compiler::ChartDocument;

use crate::command::{Command, CommandError};
use crate::document::{Document, LoadError};
use crate::editor::{Applied, Editor};
use crate::index::{Rect, SpatialIndex};
use crate::selection::{self, Level, Selection};
use crate::tool::{self, Context, Cursor, Input, Overlay, SelectTool, Tool, ToolId};

#[derive(Debug)]
pub struct Session {
    editor: Editor,
    index: SpatialIndex,
    selection: Selection,
    floor: Option<u32>,
    tool: Box<dyn Tool>,
    tool_id: ToolId,
}

impl Session {
    pub fn load(document: ChartDocument) -> Result<Self, LoadError> {
        let editor = Editor::load(document)?;
        let floor = editor.document().floors().first().map(|f| f.id);
        let mut session = Self {
            editor,
            index: SpatialIndex::new(),
            selection: Selection::default(),
            floor,
            tool: Box::new(SelectTool::default()),
            tool_id: ToolId::Select,
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
        };
        self.tool_id = id;
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
        self.with_tool(|tool, cx| tool.input(cx, &input));
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
            marquee: None,
        };
        self.tool.overlay(&mut out);
        out
    }

    fn reset_tool(&mut self) {
        self.with_tool(|tool, cx| tool.reset(cx));
    }

    fn with_tool(&mut self, f: impl FnOnce(&mut dyn Tool, &mut Context<'_>)) {
        let mut cx = Context {
            editor: &mut self.editor,
            index: &self.index,
            selection: &mut self.selection,
            floor: self.floor,
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
