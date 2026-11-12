//! Every session call the module exports, in one table (07 §2.4). The
//! dispatcher that decodes a call's arguments and encodes its reply is
//! generated from it by the `calls!` macro, and `cargo xtask types` reads
//! the same table to write the TypeScript `Calls` class and `CallTable`
//! type, so the two sides cannot disagree on a name, an argument or a
//! result.
//!
//! An entry is `fn name(s, arg: Type, …) -> Result { body }`, with
//! `-> Result | Error` when the call can be refused with that error. `s` is
//! the session. Arguments arrive as a JSON array in this order.

use cavea_compiler::document::Category;
use cavea_edit::{
    AlignTo, Applied, ArrayStep, Axis, CategoriesView, Command, CommandError, Contrast, Cursor,
    Draft, Drop, End, Field, FindingPlace, FloorGuides, FloorsView, HistoryView, Input, InputResult,
    InspectorView, LayerClick, LayersPage, Level, Numbering, NumberingView, Order, Overlay,
    Preview, Progress, Rect, SeatEdit, SeatRef, SelectionView, Session, SnapSettings, StatusView,
    TextField, ToolId, ToolView, UserGuide, ValidationView, ViewName,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::Failure;

macro_rules! calls {
    ($(
        $(#[doc = $doc:literal])*
        fn $name:ident($s:ident $(, $arg:ident: $ty:ty)*) -> $ret:ty $(| $err:ty)? $body:block
    )*) => {
        /// Runs call `method` on the session with `args`, a JSON array, and
        /// returns its result as JSON.
        pub(crate) fn dispatch(
            session: &mut Session,
            method: &str,
            args: &str,
        ) -> Result<String, Failure> {
            match method {
                $(stringify!($name) => {
                    calls!(@args args; $($arg: $ty),*);
                    let $s = session;
                    let result: Result<$ret, Failure> = calls!(@run $body $(, $err)?);
                    encode(&result?)
                })*
                _ => Err(Failure::UnknownCall { name: method.to_owned() }),
            }
        }
    };
    (@args $args:ident;) => {
        let []: [u8; 0] = decode($args)?;
    };
    (@args $args:ident; $($arg:ident: $ty:ty),+) => {
        let ($($arg,)+): ($($ty,)+) = decode($args)?;
    };
    (@run $body:block) => {
        Ok($body)
    };
    (@run $body:block, $err:ty) => {{
        let result: Result<_, $err> = $body;
        result.map_err(Failure::from)
    }};
}

fn decode<T: DeserializeOwned>(args: &str) -> Result<T, Failure> {
    serde_json::from_str(args).map_err(|e| Failure::Arguments {
        message: e.to_string(),
    })
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<String, Failure> {
    serde_json::to_string(value).map_err(|e| Failure::Encode {
        message: e.to_string(),
    })
}

calls! {
    /// The views that may differ since the last call, all of them the first
    /// time. Call it once per animation frame and re-read only those
    /// (07 §2.3, D-40).
    fn changed(s) -> Vec<ViewName> { s.changed() }

    /// The chart document JSON the server stores as the draft (07 §12).
    fn export(s) -> String { s.export() }

    // Views (07 §2.3).

    /// Undo and redo for the top bar: the stable keys of the entries they
    /// would apply, and whether a gesture is open.
    fn history(s) -> HistoryView { s.history() }
    /// The chart's name, for the top bar.
    fn title(s) -> String { s.title().to_owned() }
    /// The floor switcher's list and the floor shown.
    fn floors(s) -> FloorsView { s.floors() }
    /// The status bar's numbers and the document's revision.
    fn status(s) -> StatusView { s.status() }
    /// The selection summary; its `bounds` is what "zoom to selection"
    /// frames.
    fn selection_view(s) -> SelectionView { s.selection_view() }
    /// The world box "zoom to fit" frames. Out of date with `selection`.
    fn fit(s) -> Option<Rect> { s.fit() }
    /// The tool rail and the snapping toggles.
    fn tool_view(s) -> ToolView { s.tool_view() }
    /// The inspector's fields for the selection, or the chart's settings.
    fn inspector(s) -> InspectorView { s.inspector() }
    /// `count` rows of the layers tree from row `start`, with the total.
    fn layers(s, start: u32, count: u32) -> LayersPage { s.layers(start, count) }
    /// The categories panel's list (07 §7).
    fn categories(s) -> CategoriesView { s.categories() }
    /// The numbering panel's view of the selection (07 §6).
    fn numbering(s) -> NumberingView { s.numbering() }
    /// The labels previewed, while the proposal still fits the document and
    /// the selection. Out of date with `numbering`.
    fn numbering_preview(s) -> Option<Preview> { s.numbering_preview().cloned() }
    /// The validation panel's list: exactly the compiler's when `complete`.
    fn validation(s) -> ValidationView { s.validation() }
    /// The labels of the row and seat a finding names, for a finding shown.
    fn finding_place(s, node: u32, row: Option<u32>, seat: Option<u32>) -> FindingPlace {
        s.finding_place(node, row, seat)
    }
    /// What the canvas draws over the chart now, in chart units.
    fn overlay(s) -> Overlay { s.overlay() }
    /// The text field the tool asks for: the overlay's `text` alone.
    fn text_field(s) -> Option<TextField> { s.text_field() }
    /// The pointer the canvas shows.
    fn cursor(s) -> Cursor { s.cursor() }
    /// The user guides of the floor shown, in the order added.
    fn guides(s) -> Vec<UserGuide> { s.guides().to_vec() }
    /// Every floor's guides, for the shell to store beside the draft. Out
    /// of date with `guides`.
    fn saved_guides(s) -> Vec<FloorGuides> { s.saved_guides() }

    // Input and tools (07 §2.3, §4).

    /// Passes a pointer or key event to the active tool; the result says
    /// how far to pan the view.
    fn input(s, input: Input) -> InputResult { s.input(input) }
    /// Switches the tool; the old one abandons what it was doing.
    fn set_tool(s, tool: ToolId) -> () { s.set_tool(tool) }
    /// Sets the category new seats take (07 §7).
    fn set_category(s, key: Option<String>) -> () { s.set_category(key) }
    /// The whole string of the text field the overlay asked for, on every
    /// change.
    fn set_text(s, text: String) -> () { s.set_text(&text) }
    /// Turns each kind of snapping on or off and sets the grid step.
    fn set_snap(s, settings: SnapSettings) -> () { s.set_snap(settings) }

    // Selection and floors (07 §5.1, §5.6).

    /// Selects from outside the canvas, locked and hidden nodes included.
    fn select(s, level: Level, items: Vec<u32>) -> () { s.select(level, items) }
    /// Selects every item at the current level that can be picked (Ctrl+A).
    fn select_all(s) -> () { s.select_all() }
    /// Steps into the one selected item (Enter).
    fn step_in(s) -> bool { s.step_in() }
    /// Steps out one level, or clears the selection at the top (Escape).
    fn step_out(s) -> () { s.step_out() }
    /// Shows another floor and clears the selection; false for a floor the
    /// chart does not list.
    fn set_floor(s, floor: u32) -> bool { s.set_floor(floor) }
    /// Selects what a finding names and returns the world box to frame.
    fn focus(s, node: u32, row: Option<u32>, seat: Option<u32>) -> Option<Rect> {
        s.focus(node, row, seat)
    }
    /// Adds a floor at the end of the list; returns its id.
    fn add_floor(s, name: String, level: i32) -> u32 | CommandError { s.add_floor(&name, level) }
    fn rename_floor(s, id: u32, name: String) -> Applied | CommandError {
        s.rename_floor(id, &name)
    }
    /// Deletes a floor; its nodes move to `replacement`, or to the first
    /// floor when it is `null`.
    fn delete_floor(s, id: u32, replacement: Option<u32>) -> Applied | CommandError {
        s.delete_floor(id, replacement)
    }

    // History (07 §9).

    /// False when there is nothing to undo.
    fn undo(s) -> bool | CommandError { s.undo() }
    /// False when there is nothing to redo.
    fn redo(s) -> bool | CommandError { s.redo() }
    /// Applies a command (07 §2.2) as one undo entry.
    fn apply(s, command: Command) -> Applied | CommandError { s.apply(command) }

    // Editing the selection (07 §2.3 editing calls, §5).

    /// Adds (`count` above 0) or removes positions at one end of each
    /// selected row.
    fn row_seats(s, end: End, count: i32) -> Applied | CommandError { s.row_seats(end, count) }
    /// Shifts the selected rows along themselves by `delta` chart units.
    fn shift_rows(s, delta: f64) -> Applied | CommandError { s.shift_rows(delta) }
    /// Sets or clears the selected rows' label override.
    fn set_row_label(s, label: Option<String>) -> Applied | CommandError {
        s.set_row_label(label)
    }
    /// Marks the position at the world point `at` removed in each selected
    /// row.
    fn cut_aisle(s, at: [f64; 2]) -> Applied | CommandError { s.cut_aisle(at) }
    /// Inserts `count` removed positions at the world point `at` in each
    /// selected row.
    fn open_aisle(s, at: [f64; 2], count: u32) -> Applied | CommandError {
        s.open_aisle(at, count)
    }
    /// One edit to every selected seat.
    fn edit_seats(s, edit: SeatEdit) -> Applied | CommandError { s.edit_seats(edit) }
    /// Deletes what is selected, at any level.
    fn delete(s) -> Applied | CommandError { s.delete() }
    /// Sets or clears the focal point of the floor shown.
    fn set_focal_point(s, point: Option<[f64; 2]>) -> Applied | CommandError {
        s.set_focal_point(point)
    }
    fn align(s, to: AlignTo) -> Applied | CommandError { s.align(to) }
    fn distribute(s, axis: Axis) -> Applied | CommandError { s.distribute(axis) }
    fn flip(s, axis: Axis) -> Applied | CommandError { s.flip(axis) }
    /// One copy of the selected nodes moved by the world vector `offset`;
    /// the copies are selected.
    fn duplicate(s, offset: [f64; 2]) -> Applied | CommandError { s.duplicate(offset) }
    /// `copies` copies of the selected nodes, each one `step` from the one
    /// before; the copies are selected.
    fn array(s, copies: u32, step: ArrayStep) -> Applied | CommandError { s.array(copies, step) }
    fn order(s, order: Order) -> Applied | CommandError { s.order(order) }
    fn set_locked(s, locked: bool) -> Applied | CommandError { s.set_locked(locked) }
    fn set_hidden(s, hidden: bool) -> Applied | CommandError { s.set_hidden(hidden) }
    /// The selected nodes as clipboard text, or `null`.
    fn copy(s) -> Option<String> { s.copy() }
    /// Copy, then delete what was copied, as one undo entry.
    fn cut(s) -> Option<String> | CommandError { s.cut() }
    /// Pastes clipboard text with fresh ids, centred on the world point
    /// `at`, or offset from where it was copied; the nodes are selected.
    fn paste(s, text: String, at: Option<[f64; 2]>) -> Applied | CommandError {
        s.paste(&text, at)
    }
    /// A section around top-level nodes `ids`, with an explicit outline
    /// (world points) or an automatic one; returns its id.
    fn group(s, ids: Vec<u32>, outline: Option<Vec<[f64; 2]>>) -> u32 | CommandError {
        s.group(&ids, outline)
    }
    /// Returns a section's children to the top level, in place.
    fn dissolve(s, section: u32) -> () | CommandError { s.dissolve(section) }
    /// Makes a section's automatic outline explicit.
    fn convert_outline(s, id: u32) -> Applied | CommandError { s.convert_outline(id) }
    /// Creates the node a drawing tool would make from these numbers, at
    /// the end of `parent`'s children, and selects it; returns its id.
    fn insert(s, parent: Option<u32>, draft: Draft) -> u32 | CommandError {
        s.insert(parent, &draft)
    }

    // Numbering (07 §6).

    /// What `number` would do to the selection, changing nothing.
    fn preview_numbering(s, numbering: Numbering) -> Preview | CommandError {
        s.preview_numbering(&numbering).cloned()
    }
    fn cancel_numbering(s) -> () { s.cancel_numbering() }
    /// Renumbers the selected row blocks and rows as one undo entry.
    fn number(s, numbering: Numbering) -> Applied | CommandError { s.number(&numbering) }
    /// The hovered seat's full label, with the preview if one is shown.
    fn sample(s, node: u32, oid: u32) -> Option<String> { s.sample(node, oid) }
    /// The seat of a selected row block or row under a chart point, at a
    /// zoom (CSS px per chart unit): the seat `sample` describes.
    fn seat_under(s, at: [f64; 2], zoom: f64) -> Option<SeatRef> { s.seat_under(at, zoom) }

    // Categories (07 §7).

    /// Adds a category at `index`, or at the end when it is `null`.
    fn add_category(s, index: Option<u32>, category: Category) -> Applied | CommandError {
        s.add_category(index, category)
    }
    /// Replaces a category; a new key is carried to every reference.
    fn update_category(s, key: String, category: Category) -> Applied | CommandError {
        s.update_category(&key, category)
    }
    /// Moves a category to `index` in the list (counted without it).
    fn move_category(s, key: String, index: u32) -> Applied | CommandError {
        s.move_category(&key, index)
    }
    /// Moves every reference to `from` onto `into` and deletes `from`.
    fn merge_category(s, from: String, into: String) -> Applied | CommandError {
        s.merge_category(&from, &into)
    }
    /// Deletes a category; while it is named, a `replacement` is needed.
    fn delete_category(s, key: String, replacement: Option<String>) -> Applied | CommandError {
        s.delete_category(&key, replacement.as_deref())
    }
    /// A `#RRGGBB` colour's contrast against the chart backgrounds, or
    /// `null` for another string.
    fn contrast(_s, colour: String) -> Option<Contrast> { cavea_edit::contrast(&colour) }
    /// Whether two `#RRGGBB` colours are too close to tell apart.
    fn too_similar(_s, a: String, b: String) -> Option<bool> { cavea_edit::too_similar(&a, &b) }

    // Validation (07 §10).

    /// Validation work for idle time, up to `budget` (seat positions, plus
    /// one per piece); call again until `done`.
    fn validate(s, budget: u64) -> Progress { s.validate(budget) }

    // Guides (07 §5.2).

    /// Adds a guide on the floor shown; returns its index.
    fn add_guide(s, guide: UserGuide) -> usize | CommandError { s.add_guide(guide) }
    fn move_guide(s, index: usize, at: f64) -> () | CommandError { s.move_guide(index, at) }
    /// False when there is no such guide.
    fn remove_guide(s, index: usize) -> bool { s.remove_guide(index) }
    fn clear_guides(s) -> () { s.clear_guides() }
    /// Replaces every guide with ones saved before, all or nothing.
    fn restore_guides(s, saved: Vec<FloorGuides>) -> () | CommandError {
        s.restore_guides(saved)
    }

    // Layers (07 §2.3).

    fn expand_layer(s, id: u32, open: bool) -> () { s.expand_layer(id, open) }
    /// Expands the section holding `id` and returns its row, or `null`.
    fn reveal_layer(s, id: u32) -> Option<u32> { s.reveal_layer(id) }
    /// Selects from the tree; false for an unknown node.
    fn click_layer(s, id: u32, how: LayerClick) -> bool { s.click_layer(id, how) }
    fn lock_layer(s, id: u32, locked: bool) -> Applied | CommandError { s.lock_layer(id, locked) }
    fn hide_layer(s, id: u32, hidden: bool) -> Applied | CommandError { s.hide_layer(id, hidden) }
    /// Moves nodes `ids` to `drop` as one undo entry, keeping every seat
    /// where it is on the canvas; the moved nodes are selected.
    fn move_layers(s, ids: Vec<u32>, drop: Drop) -> Applied | CommandError {
        s.move_layers(&ids, drop)
    }

    // Inspector (07 §2.3, §9).

    /// Sets one field on every selected node, or the chart, as one undo
    /// entry; inside a scrub, replaces the last value shown.
    fn set_field(s, field: Field) -> Applied | CommandError { s.set_field(&field) }
    /// Opens a scrub: the `setField` calls until `endScrub` are one entry.
    fn begin_scrub(s) -> () | CommandError { s.begin_scrub() }
    fn end_scrub(s) -> () { s.end_scrub() }
    /// Restores the start of the scrub, leaving no entry (Escape).
    fn cancel_scrub(s) -> () { s.cancel_scrub() }
}
