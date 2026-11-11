//! A designer session (`docs/07-designer.md` §2.3): the editor, the spatial
//! index, the selection, the floor shown and the active tool. Everything
//! from outside, input or command, goes through it, so the index and the
//! selection are brought up to date after every change.

use cavea_compiler::ChartDocument;
use cavea_compiler::document::Category;

use crate::arrange::{self, AlignTo, ArrayStep, Order};
use crate::categories::{self, CategoriesView};
use crate::clipboard;
use crate::command::{Axis, Command, CommandError, End, Property, SeatEdit};
use crate::document::{Document, LoadError};
use crate::draft::Draft;
use crate::editor::{Applied, Editor};
use crate::guides::{FloorGuides, Guides, UserGuide};
use crate::import::{self, ImportError};
use crate::index::{P, Rect, SpatialIndex};
use crate::layers::Layers;
use crate::numbering::{self, Numbering, NumberingView, Preview};
use crate::rows;
use crate::section;
use crate::selection::{self, Level, Selection, View, floor_of};
use crate::tool::handles::targets;
use crate::tool::snap::{Snap, SnapSettings};
use crate::tool::{
    self, ArcTool, BlockTool, BrushTool, Button, Context, Cursor, FocalTool, HandTool, Input, Key,
    Overlay, RowTool, SelectTool, ShapeKindTool, ShapeTool, TextTool, Tool, ToolId,
};
use crate::validation::{self, Progress, ValidationView, Validator};
use crate::views::Seen;
use cavea_compiler::document::{Node, Outline, Points};

mod shell;

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
    /// The numbering panel's proposal, while it is shown (07 §6).
    preview: Option<Proposal>,
    validator: Validator,
    guides: Guides,
    layers: Layers,
    /// The tool Space returns to while it holds the hand tool.
    held: Option<ToolId>,
    /// The screen position of a middle-button pan in progress.
    middle: Option<P>,
    /// A number field is being scrubbed, in a gesture of the session's own,
    /// with the last command it showed.
    scrub: Option<Option<Command>>,
    watch: Watch,
}

/// What `changed` compares the views' sources with (07 §14).
#[derive(Debug, Default)]
struct Watch {
    seen: Seen,
    /// The selection when last compared, and a count of its changes.
    selection: Selection,
    selections: u64,
    /// Changes of the guides, the numbering preview, and the tool, active
    /// category or snap settings; inputs passed to the tool.
    guides: u64,
    previews: u64,
    tools: u64,
    inputs: u64,
}

/// What the shell does after an input: pan the view by `pan` CSS pixels
/// (the hand tool, Space, the middle button).
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputResult {
    pub pan: Option<[f64; 2]>,
}

/// The tool rail and the snapping toggles.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolView {
    pub tool: ToolId,
    /// While Space holds the hand tool, the tool it returns to.
    pub held: Option<ToolId>,
    /// The active category (07 §7).
    pub category: Option<String>,
    pub snap: SnapSettings,
}

/// A numbering previewed: for the selection and document it was made on.
#[derive(Debug)]
struct Proposal {
    selection: Selection,
    revision: u64,
    nodes: Vec<(u32, Node)>,
    overflow: cavea_compiler::document::AlphaOverflow,
    preview: Preview,
}

impl Session {
    pub fn load(document: ChartDocument) -> Result<Self, LoadError> {
        Ok(Self::open(Editor::load(document)?))
    }

    /// Opens chart document JSON, read as hostile input (07 §12): the
    /// designer's import and the way the shell opens a draft. Importing
    /// over an open chart is a new session: its undo history, selection,
    /// guides and validation start afresh, and the shell asks first.
    pub fn import(text: &str) -> Result<Self, ImportError> {
        Ok(Self::open(Editor::from_document(import::import(text)?)))
    }

    /// The chart document JSON the server stores as the draft (07 §12).
    pub fn export(&self) -> String {
        import::export(self.document())
    }

    fn open(editor: Editor) -> Self {
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
            preview: None,
            validator: Validator::default(),
            guides: Guides::default(),
            layers: Layers::default(),
            held: None,
            middle: None,
            scrub: None,
            watch: Watch::default(),
        };
        session.settle();
        session
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
        self.held = None;
        self.switch(id);
    }

    fn switch(&mut self, id: ToolId) {
        self.reset_tool();
        self.watch.tools += 1;
        self.tool = match id {
            ToolId::Select => Box::new(SelectTool::default()),
            ToolId::Hand => Box::new(HandTool::default()),
            ToolId::RowBlock => Box::new(BlockTool::default()),
            ToolId::ArcBlock => Box::new(ArcTool::default()),
            ToolId::Row => Box::new(RowTool::default()),
            ToolId::Rect => Box::new(ShapeTool::new(ShapeKindTool::Rect)),
            ToolId::Ellipse => Box::new(ShapeTool::new(ShapeKindTool::Ellipse)),
            ToolId::Polygon => Box::new(ShapeTool::new(ShapeKindTool::Polygon)),
            ToolId::Line => Box::new(ShapeTool::new(ShapeKindTool::Line)),
            ToolId::Text => Box::new(TextTool::default()),
            ToolId::Section => Box::new(ShapeTool::new(ShapeKindTool::Section)),
            ToolId::Brush => Box::new(BrushTool::default()),
            ToolId::Focal => Box::new(FocalTool::default()),
        };
        self.tool_id = id;
    }

    /// The category new seats take; at load, the chart's first.
    pub fn category(&self) -> Option<&str> {
        self.category.as_deref()
    }

    pub fn set_category(&mut self, key: Option<String>) {
        self.watch.tools += 1;
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

    /// Runs an edit built from the selection, as one undo entry; one that
    /// does not fit the selection does nothing. `then` names the selection
    /// to set after it applied.
    fn edit<T>(
        &mut self,
        f: impl FnOnce(&View<'_>, &Selection) -> Result<Option<(Command, T)>, CommandError>,
        then: impl FnOnce(&mut Selection, &Document, T, &Applied),
    ) -> Result<Applied, CommandError> {
        self.reset_tool();
        let Some((command, next)) = f(&self.view(), &self.selection)? else {
            return Ok(Applied::default());
        };
        let result = self.editor.apply(command);
        self.settle();
        if let Ok(applied) = &result {
            then(&mut self.selection, self.editor.document(), next, applied);
        }
        result
    }

    fn edit_with(
        &mut self,
        f: impl FnOnce(&View<'_>, &Selection) -> Option<Command>,
    ) -> Result<Applied, CommandError> {
        self.edit(|v, s| Ok(f(v, s).map(|c| (c, ()))), |_, _, (), _| {})
    }

    fn edit_with_result(
        &mut self,
        f: impl FnOnce(&View<'_>, &Selection) -> Result<Option<Command>, CommandError>,
    ) -> Result<Applied, CommandError> {
        self.edit(|v, s| Ok(f(v, s)?.map(|c| (c, ()))), |_, _, (), _| {})
    }

    fn view(&self) -> View<'_> {
        View {
            doc: self.editor.document(),
            index: &self.index,
            floor: self.floor,
            tol: 0.0,
        }
    }

    /// Adds (`count` above 0) or removes positions at one end of each
    /// selected row (07 §5.3).
    pub fn row_seats(&mut self, end: End, count: i32) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| rows::row_seats(v, s, end, count))
    }

    /// Shifts the selected rows sideways by `delta` chart units.
    pub fn shift_rows(&mut self, delta: f64) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| rows::shift(v, s, delta))
    }

    /// Sets or clears the selected rows' label override.
    pub fn set_row_label(&mut self, label: Option<String>) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| rows::label(v, s, label))
    }

    /// Marks the position at the world point `at` removed in each selected
    /// row (07 §5.3: an aisle that renumbers nothing).
    pub fn cut_aisle(&mut self, at: [f64; 2]) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| rows::cut_aisle(v, s, at))
    }

    /// Inserts `count` removed positions at the world point `at` in each
    /// selected row (an aisle that removes no seat).
    pub fn open_aisle(&mut self, at: [f64; 2], count: u32) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| rows::open_aisle(v, s, at, count))
    }

    /// One edit to every selected seat (07 §5.3, seat-edit mode).
    pub fn edit_seats(&mut self, edit: SeatEdit) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| rows::edit(v, s, edit))
    }

    /// Deletes what is selected, as Delete does at each level (07 §8).
    pub fn delete(&mut self) -> Result<Applied, CommandError> {
        self.edit(
            |v, s| Ok(rows::delete(v, s).map(|(c, l, i)| (c, (l, i)))),
            |sel, doc, (level, items), _| sel.set(doc, level, items),
        )
    }

    /// Sets (or clears) the focal point of the floor shown (07 §4, §5.6).
    pub fn set_focal_point(&mut self, point: Option<[f64; 2]>) -> Result<Applied, CommandError> {
        let floor = self.floor;
        self.edit_with(|v, _| tool::focal::command(v.doc, floor, point))
    }

    pub fn align(&mut self, to: AlignTo) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| arrange::align(v, s, to))
    }

    pub fn distribute(&mut self, axis: Axis) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| arrange::distribute(v, s, axis))
    }

    pub fn flip(&mut self, axis: Axis) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| arrange::flip(v, s, axis))
    }

    /// One copy of the selected nodes moved by the world vector `offset`
    /// (Ctrl+D uses [`crate::DUPLICATE_OFFSET`] each way); the copies are
    /// selected.
    pub fn duplicate(&mut self, offset: [f64; 2]) -> Result<Applied, CommandError> {
        self.edit(|v, s| arrange::duplicate(v, s, offset), select_created)
    }

    /// `copies` copies of the selected nodes, each one `step` from the one
    /// before; the copies are selected.
    pub fn array(&mut self, copies: u32, step: ArrayStep) -> Result<Applied, CommandError> {
        self.edit(|v, s| arrange::array(v, s, copies, step), select_created)
    }

    pub fn order(&mut self, order: Order) -> Result<Applied, CommandError> {
        self.edit_with(|v, s| arrange::order(v, s, order))
    }

    pub fn set_locked(&mut self, locked: bool) -> Result<Applied, CommandError> {
        self.edit_with(|_, s| arrange::set_flag(s, Property::Locked(locked)))
    }

    pub fn set_hidden(&mut self, hidden: bool) -> Result<Applied, CommandError> {
        self.edit_with(|_, s| arrange::set_flag(s, Property::Hidden(hidden)))
    }

    /// The selected nodes as clipboard text (§2.3), or `None`.
    pub fn copy(&mut self) -> Option<String> {
        self.reset_tool();
        clipboard::copy(&self.view(), &self.selection)
    }

    /// Copy, then delete what was copied, as one undo entry.
    pub fn cut(&mut self) -> Result<Option<String>, CommandError> {
        let Some(text) = self.copy() else {
            return Ok(None);
        };
        self.edit_with(|v, s| targets(v, s).map(|(ids, _)| Command::DeleteNodes { ids }))?;
        Ok(Some(text))
    }

    /// Pastes clipboard text with fresh ids, centred on the world point
    /// `at`, or offset from where it was copied; the nodes are selected.
    pub fn paste(&mut self, text: &str, at: Option<[f64; 2]>) -> Result<Applied, CommandError> {
        self.edit(
            |v, s| clipboard::paste(v, s, text, at).map(Some),
            select_created,
        )
    }

    /// The numbering panel's view of the selection (07 §6).
    pub fn numbering(&self) -> NumberingView {
        numbering::view(&self.view(), &self.selection)
    }

    /// Shows what `numbering` would do to the selected blocks and rows,
    /// changing nothing: the labels are [`Self::numbering_preview`] until
    /// the document or the selection changes, or anything is applied.
    /// Refused as [`Self::number`] would be.
    pub fn preview_numbering(&mut self, numbering: &Numbering) -> Result<&Preview, CommandError> {
        self.reset_tool();
        let view = self.view();
        let targets = numbering::targets(&view, &self.selection);
        let nodes = numbering::renumbered(&view, &targets, numbering)?;
        let overflow = numbering
            .after_z
            .unwrap_or(self.document().settings().alpha_overflow);
        let preview = numbering::preview(&view, &nodes, overflow);
        self.watch.previews += 1;
        let proposal = self.preview.insert(Proposal {
            selection: self.selection.clone(),
            revision: self.document().revision(),
            nodes,
            overflow,
            preview,
        });
        Ok(&proposal.preview)
    }

    /// The labels previewed, while the proposal still fits the document
    /// and the selection.
    pub fn numbering_preview(&self) -> Option<&Preview> {
        self.proposal().map(|p| &p.preview)
    }

    fn proposal(&self) -> Option<&Proposal> {
        self.preview
            .as_ref()
            .filter(|p| p.revision == self.document().revision() && p.selection == self.selection)
    }

    pub fn cancel_numbering(&mut self) {
        self.preview = None;
    }

    /// Renumbers the selected row blocks and rows (07 §6), as one undo
    /// entry. Nothing selected to number does nothing.
    pub fn number(&mut self, numbering: &Numbering) -> Result<Applied, CommandError> {
        self.edit_with_result(|v, s| {
            let targets = numbering::targets(v, s);
            let nodes = numbering::renumbered(v, &targets, numbering)?;
            Ok(numbering::command(v.doc, &nodes, numbering))
        })
    }

    /// The full label of seat `oid` of node `node` under the chart's label
    /// template, with the numbering previewed if there is one: the hovered
    /// seat's sample (07 §6).
    pub fn sample(&self, node: u32, oid: u32) -> Option<String> {
        let doc = self.document();
        let proposal = self.proposal();
        let previewed = proposal.and_then(|p| p.nodes.iter().find(|(id, _)| *id == node));
        let overflow = proposal.map_or(doc.settings().alpha_overflow, |p| p.overflow);
        let value = previewed.map(|(_, n)| n).or(doc.node(node))?;
        numbering::sample(doc, node, value, oid, overflow)
    }

    /// The categories panel's list (07 §7).
    pub fn categories(&self) -> CategoriesView {
        categories::view(self.document(), &self.index, self.category.as_deref())
    }

    /// Adds a category at `index`, or at the end.
    pub fn add_category(
        &mut self,
        index: Option<u32>,
        category: Category,
    ) -> Result<Applied, CommandError> {
        let index = index.unwrap_or(self.document().categories().len() as u32);
        self.apply(Command::AddCategory { index, category })
    }

    /// Replaces a category: its label, colour and accessible marker, or its
    /// key, which every reference follows, the active category included.
    pub fn update_category(
        &mut self,
        key: &str,
        category: Category,
    ) -> Result<Applied, CommandError> {
        let new = category.key.clone();
        let was = self.category.as_deref() == Some(key);
        let key = key.to_owned();
        let result = self.apply(Command::UpdateCategory { key, category });
        self.follow(was, Some(new), &result);
        result
    }

    /// Moves a category to `index` in the list (counted without it).
    pub fn move_category(&mut self, key: &str, index: u32) -> Result<Applied, CommandError> {
        let key = key.to_owned();
        self.apply(Command::MoveCategory { key, index })
    }

    /// Merges category `from` into `into`: every node and seat override
    /// naming `from` names `into`, and `from` is deleted, as one undo entry.
    pub fn merge_category(&mut self, from: &str, into: &str) -> Result<Applied, CommandError> {
        self.delete_category(from, Some(into))
    }

    /// Deletes a category. While nodes or seat overrides name it, a
    /// `replacement` is needed (they move to it, as a merge); without one
    /// the deletion is refused with [`CommandError::CategoryInUse`].
    pub fn delete_category(
        &mut self,
        key: &str,
        replacement: Option<&str>,
    ) -> Result<Applied, CommandError> {
        let references = categories::references(self.document(), key);
        if replacement.is_none() && references > 0 {
            self.reset_tool();
            let key = key.to_owned();
            return Err(CommandError::CategoryInUse { key, references });
        }
        let replacement = replacement.map(str::to_owned);
        let was = self.category.as_deref() == Some(key);
        let result = self.apply(Command::RemoveCategory {
            key: key.to_owned(),
            replacement: replacement.clone(),
        });
        let next =
            replacement.or_else(|| self.document().categories().first().map(|c| c.key.clone()));
        self.follow(was, next, &result);
        result
    }

    /// The active category follows a category renamed, merged or deleted
    /// (`was`: it was the active one).
    fn follow(&mut self, was: bool, next: Option<String>, result: &Result<Applied, CommandError>) {
        if result.is_ok() && was {
            self.watch.tools += 1;
            self.category = next;
        }
    }

    /// Validation work for idle time (07 §10, §14): pieces until their cost
    /// reaches `budget` (seat positions compiled, plus one per piece), at
    /// least one. Each piece is indivisible; an edit between steps makes the
    /// next step start from what changed.
    pub fn validate(&mut self, budget: u64) -> Progress {
        self.validator
            .step(self.editor.document(), &self.index, budget)
    }

    /// The validation panel's list: exactly the compiler's when `complete`.
    pub fn validation(&self) -> ValidationView {
        self.validator.view(self.document())
    }

    /// Selects what a finding names, on its floor (shown if it is not), and
    /// returns the world box to frame: a seat, a block's row, or the node.
    pub fn focus(&mut self, node: u32, row: Option<u32>, seat: Option<u32>) -> Option<Rect> {
        self.reset_tool();
        let doc = self.editor.document();
        let n = doc.node(node)?;
        let floor = floor_of(doc, node);
        if floor != self.floor {
            self.floor = floor;
        }
        let pad = self.index.seat_pad(node);
        let world = |pos| self.index.to_world(node, pos);
        let seats = self.index.seats(node);
        let row_index = match (n, row) {
            (Node::RowBlock(b), Some(rid)) => b.rows.iter().position(|r| r.rid == rid),
            _ => None,
        };
        let (level, item, bounds) =
            if let Some(s) = seat.and_then(|o| seats.iter().find(|s| s.oid == o)) {
                let p = world(s.pos)?;
                (Level::Seats(node), s.oid, Some(Rect::new(p, p).grow(pad)))
            } else if let (Some(i), Some(rid)) = (row_index, row) {
                let pts = seats
                    .iter()
                    .filter(|s| s.row as usize == i)
                    .filter_map(|s| world(s.pos));
                (
                    Level::Rows(node),
                    rid,
                    Rect::around(pts).map(|r| r.grow(pad)),
                )
            } else {
                let level = doc.parent(node).map_or(Level::Top, Level::Section);
                (level, node, self.index.bounds(node))
            };
        self.selection.set(self.editor.document(), level, [item]);
        bounds
    }

    /// The user guides of the floor shown (07 §5.2), in the order added.
    pub fn guides(&self) -> &[UserGuide] {
        self.guides.on(self.floor)
    }

    /// Adds a guide on the floor shown; returns its index.
    pub fn add_guide(&mut self, guide: UserGuide) -> Result<usize, CommandError> {
        self.watch.guides += 1;
        self.guides.add(self.floor, guide)
    }

    /// Moves guide `index` of the floor shown to `at`.
    pub fn move_guide(&mut self, index: usize, at: f64) -> Result<(), CommandError> {
        self.watch.guides += 1;
        self.guides.set(self.floor, index, at)
    }

    /// Removes guide `index` of the floor shown (dragged back to a ruler).
    pub fn remove_guide(&mut self, index: usize) -> bool {
        self.watch.guides += 1;
        self.guides.remove(self.floor, index)
    }

    /// Removes every guide of the floor shown.
    pub fn clear_guides(&mut self) {
        self.watch.guides += 1;
        self.guides.clear(self.floor);
    }

    /// Every floor's guides, for the shell to store beside the draft.
    pub fn saved_guides(&self) -> Vec<FloorGuides> {
        let doc = self.document();
        let none = doc.floors().is_empty();
        self.guides.saved(|f| match f {
            None => none,
            Some(id) => doc.floors().iter().any(|l| l.id == id),
        })
    }

    /// Replaces every guide with ones saved before, all or nothing.
    pub fn restore_guides(&mut self, saved: Vec<FloorGuides>) -> Result<(), CommandError> {
        self.watch.guides += 1;
        self.guides.restore(saved)
    }

    /// Passes input to the active tool. A pointer event that is not finite,
    /// or has a zoom of 0 or less, is dropped, except that such a release
    /// cancels: the drag it would have ended does not stay open.
    ///
    /// The view pans, by the result's `pan`, with the hand tool, while
    /// Space is held (the hand tool stands in for the active tool, which
    /// gets it back on release), and with the middle button in any tool.
    pub fn input(&mut self, mut input: Input) -> InputResult {
        match &input {
            Input::PointerUp(p) if !p.valid() => input = Input::PointerCancel,
            Input::PointerDown(p) | Input::PointerMove(p) if !p.valid() => {
                return InputResult::default();
            }
            _ => {}
        }
        self.end_scrub();
        self.watch.inputs += 1;
        if let Some(pan) = self.middle_pan(&input) {
            return pan;
        }
        match input {
            Input::KeyDown(Key::Char(' '), _) => {
                // Not in the middle of a gesture, which switching would cancel.
                if self.held.is_none()
                    && self.tool_id != ToolId::Hand
                    && !self.editor.gesture_open()
                {
                    self.held = Some(self.tool_id);
                    self.switch(ToolId::Hand);
                }
                return InputResult::default();
            }
            Input::KeyUp(Key::Char(' '), _) => {
                if let Some(back) = self.held.take() {
                    self.switch(back);
                }
                return InputResult::default();
            }
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
        InputResult {
            pan: self.tool.take_pan(),
        }
    }

    /// A drag with the middle button pans in any tool; the tool sees none
    /// of it.
    fn middle_pan(&mut self, input: &Input) -> Option<InputResult> {
        let pan = |from: P, to: P| InputResult {
            pan: Some([to[0] - from[0], to[1] - from[1]]),
        };
        match (input, self.middle) {
            (Input::PointerDown(p), None)
                if p.button == Button::Middle && !self.editor.gesture_open() =>
            {
                self.middle = Some(p.screen);
                Some(InputResult::default())
            }
            (Input::PointerMove(p), Some(from)) => {
                self.middle = Some(p.screen);
                Some(pan(from, p.screen))
            }
            // The tool sees the release too: a press it had is let go.
            (Input::PointerUp(p), Some(from)) => {
                self.middle = None;
                let up = pan(from, p.screen);
                self.with_tool(|tool, cx| tool.input(cx, input));
                Some(up)
            }
            (Input::PointerCancel, Some(_)) => {
                self.middle = None;
                self.with_tool(|tool, cx| tool.input(cx, input));
                Some(InputResult::default())
            }
            (Input::PointerDown(_), Some(_)) => Some(InputResult::default()),
            _ => None,
        }
    }

    pub fn snap_settings(&self) -> SnapSettings {
        self.snap.settings
    }

    /// Turns each kind of snapping on or off, and sets the grid step (a
    /// step that is not finite and above 0 turns the grid off).
    pub fn set_snap(&mut self, settings: SnapSettings) {
        self.watch.tools += 1;
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
        self.end_scrub();
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
        let focal = self.floor.and_then(|f| {
            let floors = self.document().floors().iter();
            floors.filter(|l| l.id == f).find_map(|l| l.focal_point)
        });
        let mut out = Overlay {
            focal,
            outlines,
            frame,
            sections: self.section_outlines(),
            guides: self.snap.guides.clone(),
            user_guides: self.guides.on(self.floor).to_vec(),
            markers: validation::markers(&self.validation(), &self.index, |n| {
                selection::shown(self.document(), self.floor, n)
            }),
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
        self.end_scrub();
        self.preview = None;
        self.snap.guides.clear();
        self.with_tool(|tool, cx| tool.reset(cx));
    }

    /// Runs the tool, whose own state (a marquee, a readout, its cursor) the
    /// overlay shows: every run counts as an input for `changed`.
    fn with_tool(&mut self, f: impl FnOnce(&mut dyn Tool, &mut Context<'_>)) {
        self.watch.inputs += 1;
        self.snap.user = self.guides.on(self.floor).to_vec();
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
        self.validator.sync(doc);
        self.selection.reconcile(doc);
        // The active category is one of the chart's, also after an undo or
        // redo of a category edit: else the first.
        let cats = doc.categories();
        if self
            .category
            .as_ref()
            .is_some_and(|k| !cats.iter().any(|c| &c.key == k))
        {
            self.watch.tools += 1;
            self.category = cats.first().map(|c| c.key.clone());
        }
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

#[cfg(test)]
impl Session {
    pub(crate) fn validator(&self) -> &Validator {
        &self.validator
    }
}

/// Selects what a command created, at `level`.
fn select_created(sel: &mut Selection, doc: &Document, level: Level, applied: &Applied) {
    sel.set(doc, level, applied.created.iter().copied());
}
