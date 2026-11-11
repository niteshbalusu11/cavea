# 07 · Designer

**Status:** Draft v0.1 · 2026-11-10

The designer is the browser editor in which a chart is drawn. It is the largest single piece of the product and the one a prospect judges first.

The chart document it edits is specified in `03-chart-format.md` §2.

## 1. Goals

1. A venue manager with no training draws a 2,000-seat theatre in under an hour.
2. The common case is fast: a block of rows is one drag, and numbering is one panel.
3. Nothing is lost: every change is undoable, and drafts save themselves.
4. A published chart with bookings cannot be damaged by accident.
5. It stays responsive on a 100,000-seat chart.
6. It can be embedded, white-labelled, in a customer's back office.

Desktop browsers only. Minimum viewport 1280 × 720. Tablets are P2.

## 2. Architecture

```
┌──────────────────────── Designer page ────────────────────────┐
│ SolidJS chrome: toolbar · inspector · layers · panels · menus │
│        ▲ view models (events)        │ commands               │
│        │                             ▼                        │
│  cavea-edit (WASM): document · command stack · tools ·        │
│                     snapping · validation · live compile      │
│        │ live scene                                           │
│        ▼                                                      │
│  cavea-render (same WASM module): canvas + overlay batcher    │
└───────────────────────────────────────────────────────────────┘
```

- The **document lives in Rust**. The chrome never mutates it directly; it sends commands and renders view models.
- **Commands** are serialisable values (`MoveNodes`, `SetProperty`, `CreateRowBlock`, …). Each has an inverse, computed when it is applied.
- **Tools** are state machines in Rust that turn pointer and key input into commands, with a live preview while dragging.
- **View models** are small, purpose-built snapshots (selection summary, inspector fields, layer tree page, validation list) pushed to the chrome when they change.
- TypeScript types for commands and view models are generated from the Rust definitions.

### 2.1 Document model

`cavea-edit`'s `Document` holds the chart-level data (`name`, `settings`, `categories`, `floors`, `nextOid`) and an arena of nodes keyed by node id. Each node is stored as the compiler's own node value (`cavea_compiler::document::Node`), so nothing the schema defines is lost, except that a section's `children` is kept as parent and child links beside it; the child lists keep sibling order, which is draw order and document order. A lookup or a single-node edit by id costs a map lookup, whatever the seat count: a 100,000-seat chart is a few hundred nodes.

- **Load and save are exact.** `Document::load` takes a parsed `ChartDocument`; `snapshot()` rebuilds it as it stands. A document loaded and saved without edits serialises to the same JSON that `ChartDocument::to_json` gives for the original. A document with two nodes of one id is refused at load (E009), since commands could not name either.
- **Change tracking.** A revision counter rises with every change. Each node keeps the revision of its last change of any kind (content, transform, parent, children) and of its last change of content (anything but transform and place), and removed ids are kept with the revision that removed them. `changes_since(n)` lists the nodes changed and removed since revision `n` and whether chart-level data or the top-level order changed. A section's transform moves its children; they are not listed. The live scene re-expands a node when its content revision passes the one it last saw.
- **Structure.** Only top-level sections hold children, and sections are top-level (the compiler does not compile nested sections). A loaded document that nests deeper is kept as it is; commands never nest further.

### 2.2 Commands

A command is plain data, serialised as JSON tagged by `op` (camelCase field names, unknown fields refused). The designer's tools build commands in Rust; the chrome sends the same JSON. A command applies completely or returns an error and leaves the document byte for byte as it was. The editor allocates every new id; ids in a command's payload are ignored.

| `op` | Fields | Effect |
|---|---|---|
| `createNode` | `parent` (section id or `null`), `index`, `node` | Inserts the node (a section with its children) at `index` among `parent`'s children. Every id, `rid` and `oid` in it is replaced by a fresh one, in document order; each row's seats get one run; seat overrides follow their seats. Returns the new node's id |
| `deleteNodes` | `ids` | Deletes the nodes with their subtrees |
| `moveNodes` | `ids`, `delta` | Adds `delta` to each origin, in its parent's space |
| `transformNodes` | `nodes: [{id, origin, rotation}]` | Sets each origin and rotation |
| `placeNode` | `id`, `parent`, `index` | Reparents and reorders; `index` counts after the node has left its place. A node changing parent stays where it is on the canvas: its `origin` becomes its world origin carried into the new parent's space, and its `rotation` changes by the old parent's rotation minus the new one's (reduced to (−180°, 180°] when it changes), so every seat keeps its world position to within 1e-9 cu at chart scale (the turns use the platform's sine and cosine; what is stored is plain numbers). The inverse restores the old origin, rotation and place exactly |
| `setProperty` | `id`, `property: {field, value}` | One inspector field: `name`, `floor`, `locked`, `hidden` (any node); `label` (section, row, table, booth, area); `category` (those and row blocks); `priority`, `entrance`, `outline` (section); `seatPitch` (row block, row); `rowPitch`, `curve`, `align`, `stagger`, `skew`, `rowLabels` (row block); `seatLabels` (row block, row, table); `decoration` (shape, text, icon, image: the whole node, same type, id kept) |
| `setChart` | `property: {field, value}` | `name` or `settings` |
| `addCategory` | `index`, `category` | Key must be new; at most 4,095 |
| `updateCategory` | `key`, `category` | A new key is carried to every reference |
| `removeCategory` | `key`, `replacement` | References, seat overrides included, move to `replacement`, or are cleared to inherit when it is `null` |
| `addFloor` | `index`, `floor` | Its `id` is assigned: one above the largest; at most 255 |
| `updateFloor` | `floor` | Replaces the floor with that `id` |
| `removeFloor` | `id`, `replacement` | Nodes on it move to `replacement`, or to the default floor when it is `null` |
| `addRows` | `id`, `index`, `count`, `seats` | Inserts `count` rows of `seats` positions, with fresh `rid`s and oids, before row `index` of a row block. Other rows keep their `rid`s, oids and overrides; in node space the rows from `index` on move back by `count × rowPitch` (03 §2.4) |
| `removeRows` | `id`, `rids` | Removes rows with their seats' overrides. A block keeps at least one row (deleting it is `deleteNodes`) |
| `addSeats` | `id`, `rid`, `end` (`start` or `end`), `count` | Adds positions at one end with fresh oids; existing runs are kept (extended when contiguous) and `removed` positions shift with seats added in front. The kept seats stay where they are: the row's `offset` takes up the shift its alignment would cause (`offset += a × Δ − (Δ at the start, else 0)`, with `Δ = count × seatPitch` and `a` 0, ½ or 1 for left, centre, right). Only a bulge, which depends on the row's length, reshapes |
| `removeSeats` | `id`, `rid`, `end`, `count` | Removes positions at one end with their overrides, the kept seats staying in place as for `addSeats`; a row keeps at least one position. Undo restores the same oids |
| `resizeRows` | `id`, `end`, `count` | Adds (`count` above 0) or removes (below 0) positions at one end of every row of a row block, as `addSeats` or `removeSeats` would row by row, in one pass; every row keeps at least one position |
| `removeSeat`, `restoreSeat` | `id`, `oid` | Adds the seat's position to `removed`, or takes it out; the oid stays |
| `setSeatOverride` | `id`, `oid`, `value` | Sets or (`null`) clears one seat's override in a row block or row |
| `composite` | `commands` | Applies them in order as one undo step |

Commands are refused, with a typed error, for: a missing node; a `count` or `seats` of 0; a parent that is not a section, is the node or is inside it; a placement that breaks the structure rule; an index out of range; a field the node does not have; a number that is not finite (the saved document must parse again); a node over the limits of `02-domain-model.md` §14 (4,095 seats in a row; 1,000,000 bookable objects in the chart, counting seat positions, booths, areas and tables booked whole, the chart's existing ones included), checked before any id is allocated; a duplicate category key; an unknown category or floor named by a category or floor command. References to unknown categories or floors in node fields are validation findings (E007), not refusals: users make mistakes mid-edit.

Each command is translated into small internal operations (insert or remove a subtree, place a node, set a transform, swap a property value, replace the category or floor list, replace a row block's rows or one row, set a seat override, set `nextOid`), each of which returns the operation that restores the previous state exactly. Operations carry ids, so they are never accepted from outside.

### 2.3 Tools and the session

A `Session` holds the editor, the spatial index (§5.2), the selection (§5.1), the floor shown and the active tool. Everything from outside goes through it: input for the tool, and commands, undo, redo and selection changes from the chrome. After each, the index is synced from change tracking and the selection is reconciled with the document. A command, undo, redo or selection change from the chrome first makes the active tool abandon what it is doing, as Escape would.

A tool is a state machine implementing one small interface:

- **Input.** Pointer down, move and up, each with its position in chart units and in CSS pixels, the zoom (CSS pixels per chart unit, to turn screen tolerances into chart units), the button, the pointer kind (mouse, touch, pen), the modifiers (Shift, Alt, Ctrl, which is Cmd on macOS) and a timestamp in milliseconds; pointer cancel; key down and up with modifiers. The session drops a pointer down or move with a non-finite number or a zoom of 0 or less, and turns such a release into a cancel, so a drag never outlives its pointer. A release or move without a press, a second press during a drag and a key repeated any number of times are expected and do bounded work.
- **What it may do.** Change the selection; apply commands to the editor, opening a gesture when a drag starts, applying a command on each move (the document changes at once; the live scene redraws it) and closing it on release as one undo entry, or cancelling it on Escape; report a cursor; add overlay data.
- **Reset.** Abandons the tool's state as Escape would: cancels the gesture it opened and restores the selection from before an unfinished press or box. The session calls it before switching tools and before anything from the chrome, so no gesture is ever left open by a tool.
- **Overlay.** Plain data in chart units for the canvas's overlay batcher (`04-renderer.md` §25): one outline per selected item, following its node's rotation (a node's box in its own space, a row's box around its seats, a seat's box); the frame around the whole selection (the item's own outline for one item, else the bounds of the outlines), where handles attach; the selection box being dragged; the select tool's handles (§5.1), each a kind, a point in chart units and an offset in CSS pixels (the rotate handle sits `ROTATE_OFFSET` px outside the frame, whatever the zoom); the live readout of the gesture in progress for the status bar (rows × seats and pitches, an arc's radius, span, rows and seats, an angle, a width and height, a length and angle); and the numbers typed so far (§4). Screen-space sizes and thresholds are constants in `cavea_edit::tuning`, with `LIVE_SEATS`, the most seats one drag may preview.
- **Drawing.** A drawing tool shows its node from the start of the gesture: the gesture holds the result of one `createNode` (or one resize command) on the state at its start, and each change of what the drag shows rewinds the gesture to its start, id counter included, and applies the new command in it. The node being drawn so keeps the same ids, and the document holds exactly what the pointer shows now, whatever path it took. Only the tools rewind; outside them a gesture can only be ended or cancelled. Release closes the gesture as one undo entry; Escape leaves the document byte for byte as it was, and the ids the drawing took stay taken (§9). Drawing tools stay armed after each node and leave it selected; Escape with nothing in progress does nothing (the chrome may switch to the select tool).
- **Where a node goes.** At the top level, on the floor shown (the node names it unless it is the first floor). When the selection is a section's children (the user stepped in) and the press is on that section (§5.1) or the section is empty, the node goes into the section, drawn in its space. Seats take the session's active category, which is the chart's first category at load until the chrome sets another (§7).
- **Without a pointer.** `Session::insert(parent, draft)` takes a `Draft` in numbers (a straight block: front-row centre, rows, seats, pitches; an arc block: centre, radius, apex direction, rows, front-row seats, pitches; a single row: start, end, an optional bend point, pitch; a rectangle or ellipse: centre and size; a line: its ends; a polygon: its corners; a text: position and string) and creates, as one undo entry, exactly the node the tool would, at the end of `parent`'s children, and selects it (§15). The tools build their nodes with the same function, which refuses what the compiler would reject. Sections have their own calls (§5.5): `Session::group(ids, outline)`, `Session::dissolve(id)` and `Session::convert_outline(id)`.
- **Text input.** The Rust tool owns no text field. While text is typed the overlay carries `text`: the node being edited once it exists, the field's position in chart units and the string so far; the chrome shows a field there and sends the whole string with `Session::set_text` on every change. Each string is shown at once, inside the tool's gesture; Enter or a press anywhere commits it as one undo entry, Escape (or anything that resets the tool) cancels it. The field closes when the overlay's `text` is gone.
- **Snapping.** The session holds the snap settings (`Session::set_snap`: each kind on or off, and the grid step) and passes them to the tools, which pull pointer positions, moves and angles onto targets (§5.2). The overlay's `guides` describe what the last pointer event snapped to; they are cleared before each input and after a release.
- **Sections in the overlay.** The overlay's `sections` holds, in chart coordinates, the outline of each selected section and of the section whose children are selected: the explicit points, or the automatic outline computed now with the compiler's algorithm (§5.5).

Tools have no clock and do no I/O, and every order they produce comes from sorted sets, so the same input gives the same document, selection and overlay. A new tool is a type implementing the interface and a `ToolId` the session maps to it.

## 3. Layout

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ ☰  Royal Theatre ▾   ↶ ↷   Saved 12:04      Floor: Stalls ▾   Preview  Publish│  top bar
├────┬───────────────────────────────────────────────────────────┬──────────────┤
│ ▸  │                                                           │ INSPECTOR    │
│ ✋ │                                                           │ Row block    │
│ ▦  │                                                           │ Rows   12    │
│ ◠  │                       CANVAS                              │ Seats  30    │
│ ⬠  │                                                           │ Pitch  55    │
│ ◯  │                                                           │ Curve  ◠ 24m │
│ ▭  │                                                           │ Category ▾   │
│ T  │                                                           │ Row labels ▾ │
│ 🖼 │                                                           │ Seat labels ▾│
│ ⌖  │                                                           ├──────────────┤
│    │                                                           │ LAYERS       │
│    │                                                           │ ▾ Orchestra  │
│    │                                                           │   ▸ Block A  │
├────┴───────────────────────────────────────────────────────────┴──────────────┤
│ 2,148 seats · 3 warnings ▸        x 1240  y 380        Snap ✓  Grid ✓   82% ▾ │  status bar
└───────────────────────────────────────────────────────────────────────────────┘
```

| Region | Contents |
|---|---|
| Top bar | Menu, chart name, undo and redo, save state, floor switcher, preview, publish |
| Tool rail (left) | One tool active at a time |
| Canvas | The chart, overlays, rulers (optional) |
| Inspector (right, top) | Properties of the selection; chart settings when nothing is selected |
| Panels (right, bottom, tabbed) | Layers, Categories, Validation |
| Status bar | Seat count, validation summary, cursor position, snapping toggles, zoom |

Panels are resizable and collapsible. Layout preferences persist per user.

## 4. Tools

| Tool | Key | Gesture | Result |
|---|---|---|---|
| Select | V | Click, shift-click, drag a box | Selection. Drag to move; handles to rotate and resize |
| Hand | H, or hold Space | Drag | Pan |
| Row block | R | Drag a rectangle | Rows × seats at the default pitch; counts shown live while dragging |
| Arc block | A | Click the centre, drag the first row's radius and angle span, drag outward for rows | Concentric curved rows |
| Single row | Shift+R | Click start, click end; optional third click bends it | One row along a line or curve |
| Section | S | Click points, or drag a box around existing blocks | A section containing what is inside |
| Table | T | Click or drag | Round or rectangular table with seats |
| Booth | B | Drag | Bookable rectangle |
| Area | G | Drag a rectangle or click a polygon | General-admission area |
| Shape | U | Drag; submenu: rectangle, ellipse, polygon, line | Decoration |
| Text | X | Click, type | Text object |
| Image | I | Pick a file, place | Reference or background image |
| Icon | — | Pick from a palette, click | Entrance, exit, stairs, lift, toilets, bar |
| Focal point | F | Click | Sets the floor's focal point |
| Category brush | C | Click or drag over seats, rows or blocks | Assigns the active category |
| Seat flags | — | Inside seat-edit mode | Accessible, companion, restricted view, aisle |
| Measure | M | Drag | Distance and angle readout |

Common to all tools:

- Escape cancels the gesture in progress.
- Shift constrains to 15° angles or to a square.
- Alt disables snapping temporarily (each event with Alt held is not snapped).
- The pointer positions the drawing tools use are snapped as points (§5.2): the presses and dragged points of the row block, single row, shape, text and section tools, polygon corners, and the arc block's centre and first press (not its sweep or rows). The node being drawn is not a target.
- Numeric entry: after starting a drag, typing a number and Enter sets the exact value shown in the live readout (rows, seats, radius). The entry is up to three fields separated by `x`, each digits with at most one point, in the readout's order; an empty field keeps the dragged value, Backspace deletes the last character, and at most 32 characters are kept. Enter applies the fields and finishes the node, as one undo entry; the rest of the drag is ignored until release. Enter with nothing typed finishes the node as drawn. Typed counts are whole, at least 1, and within `02-domain-model.md` §14 (4,095 seats in a row, the chart's remaining objects); a drag alone previews at most `LIVE_SEATS` (20,000) seats.

**Row block.** Press, drag, release. Seats per row are the drag's width over the seat pitch and rows its height over the row pitch, both rounded, at least 1, at the default pitches (55 and 95 cu, 03 §1); Shift makes the drag square first. Each seat takes a pitch-wide cell: the cells start at the press and run towards the pointer, so a drag up or left of the press works the same, and the front row (row 0) is always the top one. The block is straight and centre-aligned, its `origin` the centre of the front row, rotation 0, the active category, default labels (rows `alpha` from A front to back, seats `numeric` from 1 left to right: the scheme defaults of 03 §2.5, so two new blocks outside sections repeat labels until they are put in sections or renumbered, E001), one `oid` run per row from the id counter. It shows from the press as 1 × 1; a release before the drag threshold leaves nothing. Numeric entry: `rows`, `rows x seats` or `x seats` (`12x30` is 12 rows of 30).

**Single row.** Click the start, then click the end (or press at the start, drag and release at the end). The row then bends through the pointer; a third click fixes the bend there, and a third click within 24 CSS px of the end, or Enter, keeps it straight. The result is a `row` with `origin` at the start, rotation 0, `path` a `polyline` in that space, `seatPitch` 55, as many seats as fit along the path (`⌊length / pitch⌋ + 1`: the first on the start, the last within one pitch of the end), label `A`, default seat labels, the active category, one `oid` run. A bent row is the quadratic Bézier curve from the start to the end whose middle (`t = ½`) is the third click, flattened into `n = ⌈√(sag / 0.5)⌉` equal parameter steps (2 to 64), `sag` being the click's distance from the chord's middle: a chord of a quadratic strays from it by at most `sag / n²`, so the polyline is within 0.5 cu of the curve up to a sag of 2,048 cu; a sag under 0.5 cu is straight. A row shorter than 1 cu shows nothing; one past 4,095 seats is not shown (the last row shown stays). Escape cancels; one undo entry.

**Shapes.** Rectangle and ellipse: press, drag, release; Shift makes the drag square (a circle). The shape fills the dragged box: `origin` its centre, `size` its width and height, each at least 1 cu; a release before the drag threshold leaves nothing. Line: drag, or click both ends; Shift turns it to 15° steps about the start. `origin` is the start, `points` `[[0, 0], end − start]`, at least 1 cu long. Polygon: click the corners; a click on the first corner (within the hit tolerance, with at least three corners) or Enter closes it, Escape cancels. A corner whose new edge would cross or double back along the outline, or come within 1 cu of the last corner, is refused, as is a polygon past 4,096 corners and a closing click that would make the outline cross itself or enclose less than 1 cu² (the compiler reports a self-intersecting fill as an error); `origin` is the first corner and `points` are relative to it. Every shape has rotation 0 and `layer: below`; rectangles, ellipses and polygons are filled `#D1D5DB` with no stroke, a line has no fill and a `#6B7280` stroke 4 cu wide, so each compiles without findings. These defaults live in `draft.rs`. One undo entry each.

**Text.** Click, then type (§2.3, text input). The node is created with the first non-empty string: `origin` the click, size 60, weight 400, colour `#111827`, centred, rotation 0; at most 256 bytes (a longer string is not shown: the last one shown stays). Enter or a click elsewhere commits it as one undo entry; Escape cancels; an empty string at commit creates nothing. A click on a text node with the text tool, or a double click on one with the select tool, edits its string the same way (as one `setProperty decoration`); an existing node committed empty keeps its text.

**Section.** Drag a box, or click a polygon (a press released before the drag threshold is its first corner; the polygon then closes as the polygon tool's does, its closing click refused if the outline would cross itself). A section is always top-level, on the floor shown, and is created as one undo entry and selected (§5.5). The box makes a section holding every top-level node it fully contains, the same rule as box selection (§5.1: nodes that can be picked, by their world bounds), sections excepted (they do not nest), with `outline: "auto"`; a box holding no such node makes an empty section whose explicit outline is the box (at least 1 cu each way). The polygon makes a section whose explicit outline is the points clicked, holding every top-level node, sections excepted, that can be picked and whose outline (its box in its own space, carried into the world) has its four corners inside the polygon.

**Arc block.** Click the centre. Then press on the front row and drag along it: the radius is the pointer's distance from the centre (at least 55 cu), and the span is the angle swept from the press, unwrapped so that a drag can go past half a turn, and kept short of a full turn by one seat pitch (03 §2.4: every seat within half a turn of the apex). Release, then move outward for rows (one per row pitch beyond the front row, rounded) and click, or press, drag and release, to finish. The block is a centre-aligned `rowBlock` with `curve: arc`, `radius` the front row's radius, the apex at the middle of the span: with centre `c`, radius `r` and apex direction `φ` (clockwise from +x), `origin = c + r (cos φ, sin φ)` and `rotation = φ − 90°` (reduced to (−180°, 180°]), so the curve's centre `(0, −radius)` in node space lands on `c`. The front row has `⌊r × span / seatPitch⌋ + 1` seats, so the row runs through the release point and its end seats lie within half a pitch, along the row, of the directions of the press and the release; each row behind, of radius `Rᵢ = r + i × rowPitch`, fills the same angle with `⌊(seats − 1) × Rᵢ / r⌋ + 1` seats (rows are cut where a row would pass 4,095 seats or the cap). The direction and positions use the platform's sine and cosine on the pointer's input; what is stored is plain numbers, from which the compiler recomputes every seat with its own arithmetic. Numeric entry: `radius x seats x rows`, seats being the front row's (its span follows).

## 5. Editing

### 5.1 Selection and transform

- Click selects the top-level node (block, table, shape). Double-click enters it: first to rows, then to seats (**seat-edit mode**). Escape steps back out.
- Drag box selects what it fully contains; with Alt held, what it touches.
- Move: drag, or arrow keys (1 cu; with Shift, 10 cu).

**Selection levels.** The selection is a set of items at one level: top-level nodes; the children of one section; the rows of one row block; or the seat positions of one row block or row (seat-edit mode, removed positions included so they can be restored). It is not part of the document and not on the undo stack. After every change it drops what the document no longer holds at its level, and falls back to an empty top-level selection when the section, block or row it is in is gone; undo does not bring a selection back.

- **Click.** A press acts on the topmost item at the current level under the pointer, within 4 CSS px (mouse) or 12 (touch, pen). At the top level a node inside a section is reached through its section, so a click there selects the section, as the layers panel shows it. A press that hits nothing at the current level stays there if it is on the node holding the level (a gap between seats, the empty part of a section), and otherwise is tried at the level holding that node, out to the top. A press on an item not selected selects it alone, or with Shift adds it; a press on a selected item keeps the selection (so it can be dragged), and the release selects it alone, or with Shift takes it out. A click on empty canvas clears the selection, except with Shift.
- **Picking.** Hidden and locked nodes, nodes in a hidden or locked section and nodes on another floor than the one shown are not picked on the canvas (rows and seats through their node). The layers and validation panels can still select them.
- **Double-click.** A second press within 300 ms and 24 px of the first click's release enters the item it lands on: a section to its children, a row block to its rows, a row to its seats, a block's row to the block's seats. The item under the pointer at the new level is selected.
- **Escape** cancels a drag in progress; otherwise it steps out one level, selecting what held the items: from seats to the rows holding them (or to the row node), from rows to the block, from a section's children to the section. At the top level it clears the selection.
- **Keyboard.** Ctrl+A selects every item at the current level that can be picked; Enter steps into the one selected item, selecting everything in it; Escape as above.
- **Drag.** A press becomes a drag after 4 CSS px (mouse) or 8 (touch, pen); a press that moves less is a click. A drag starting on a selected node at a node level moves the selection; any other drag draws a box from the press, which selects live: the items its box contains or, with Alt held at the last move, touches (a block or row through its seats, a section through its children or explicit outline, other nodes through their bounds; a row when all its seats are contained or any is touched); with Shift at the press it adds to the selection. Escape during a box restores the selection from before the press.
- **Move.** The selected nodes that can be picked follow the pointer on screen, also inside a turned section: each origin is set to its start plus the pointer's travel turned into the parent's space, so the result does not drift. One gesture, one undo entry; Escape puts everything back. Each arrow key press, auto-repeat included, nudges as one undo entry. Rows and seats are not moved by dragging or nudging yet.
- Rotate: a handle outside the selection box; snaps to 15° with Shift; the pivot is draggable.
- Resize: on a row block, dragging a side handle adds or removes rows or seats; with Ctrl held it changes the pitch instead. On shapes it scales.

**Handles.** At a node level, a selection with nodes that can be picked shows a pivot, at the frame's centre until moved, and a rotate handle outside the middle of the frame's top side. One row block also shows four side handles; one rectangle, ellipse or polygon four side and four corner handles; one line a handle on each end. Several nodes show only the pivot and rotate handle; rows, seats, sections and other nodes show no resize handles. Sides and corners are named in the node's own space, y down, so they turn with it: on a row block, top is the front (row 0), bottom the back, left the end holding each row's first position, right the other end, whether the block is turned or curved. A press within 6 CSS px of a handle (14 for touch and pen) takes it once it becomes a drag; a release before the drag threshold is an ordinary click there, so a click or double click on a node's centre still selects and enters it, while a drag from it moves the pivot.

**Rotate.** Dragging the rotate handle turns the selection about the pivot by the angle the pointer sweeps about it, reduced to (−180°, 180°]; Shift rounds that angle to a multiple of 15°. Each node's origin moves on its circle about the pivot (in its parent's space, so a turned section works) and its rotation changes by the same angle, reduced to (−180°, 180°]; a section's children follow it unchanged. One gesture, one undo entry; Escape restores. The turn's sine and cosine are the platform's, applied to the stored origins at the start, so the result does not drift; turning by a multiple of 15° and back restores each rotation exactly and each origin to within 1e-9 cu at chart scale. A moved pivot stays for that selection (a drag of the selection carries it) and is forgotten when the selection changes.

**Resize a row block.** The handle's travel is measured along the block's own axes from the press, in whole rows or seats (rounded), with the count in the readout. Bottom adds or removes rows at the back (`addRows` with the back row's seat count, `removeRows`); top does so at the front and moves `origin` by the rows added along the block's −y (and along its x by the skew's shear of that distance, `travel × tan(skew)`), and a curved block's radius by the same amount, so every kept row and an arc's centre stay where they were. Left and right add or remove positions at that end of every row in one `resizeRows`, which keeps every kept seat in place, oids and overrides included, whatever the alignment; only a bulge, which depends on the row's length, reshapes (§2.2). On a staggered block the top handle adds or removes rows in pairs: an odd count would change which kept rows are odd, and so shift them by the stagger. A block keeps at least one row and each row one seat; additions stop at 4,095 seats in a row, the chart's remaining objects and `LIVE_SEATS` per drag; on an arc, at the last row or seat the compiler accepts (no row's radius reaching zero or changing sign, no seat past half a turn from the apex, checked here with the compiler's rule). With Ctrl the same handles change the pitch, in whole chart units of at least 1, so that the dragged side follows the pointer: bottom and top change the row pitch, the back row staying for top (the origin, with the skew's shear, and an arc's radius move with the front row); left and right change the seat pitch, the widest row's far end staying for a straight block (the origin moves along the block's x by the alignment's share) and the apex staying for an arc. A pitch change the arc cannot take is not shown. Each resize is one gesture from the block as it was at the press, one undo entry.

**Resize a shape.** A corner or side handle scales the shape about the opposite corner or side, in its own space; a corner with Shift keeps the aspect ratio. Each side stays at least 1 cu, and dragging past the opposite side stops there. A rectangle or ellipse gets a new `size` and an `origin` moved to the new box's centre; a polygon's points are scaled as a whole. A line's end handle moves that end (Shift: 15° steps about the other end), at least 1 cu from it. A `path` shape shows no resize handles. One gesture, one undo entry, as one `setProperty` `decoration`.
- Flip horizontal and vertical; duplicate (Ctrl+D); copy and paste, including between charts.
- Array duplicate: repeat the selection *n* times with an offset or around a centre.
- Align (left, centre, right, top, middle, bottom) and distribute (horizontal, vertical, equal gaps).
- Group and ungroup decoration nodes. Lock and hide any node.
- Order: bring forward, send backward, for decoration nodes.

### 5.2 Snapping and guides

| Snap target | Shown as |
|---|---|
| Grid (configurable step, default 5 cu) | Dots at high zoom |
| Edges and centres of other nodes | Magenta guide lines |
| Equal spacing between neighbours | Spacing markers with the distance |
| Row alignment (a block's rows lining up with a neighbour's) | Dashed row extensions |
| Angles (15°), and the focal point direction | Angle arc |
| User guides dragged from the rulers | Cyan lines |

Snapping considers only nodes near the viewport, through a spatial index kept by the edit core.

**Rules.** A target pulls a position when it is within 6 CSS px (`tuning::SNAP`, divided by the zoom). The targets are the nodes drawn on the floor shown (not hidden, nor in a hidden section; locked ones count) whose world bounds lie within 1,000 CSS px (`tuning::SNAP_REACH`, about a viewport) of the pointer or of the moved selection, except what is being moved, drawn or resized, what it holds, the section holding it, and the section a new node is drawn into. X and Y snap independently. Of the targets in reach on an axis the best kind wins, in this order: row alignment, edges and centres (and row ends), equal spacing, the grid; within a kind the nearest, then the smaller coordinate. A target chosen at one event is kept at the next while it is still in reach and no better kind is (a held grid line, 15° step or row line counts even when it is no longer the nearest one), so a pointer jittering between two targets does not flicker; each press starts afresh. Alt held turns snapping off for that event; `Session::set_snap` turns each kind off (`grid` with its `step`, default 5 cu, from the session, `objects`, `spacing`, `rows`, `angles`). Snapping only adjusts the position the gesture would use: it moves nothing else, and what a gesture commits is its final position, as without snapping. A correction is at most the threshold, so a finite position stays finite.

- **Points** (drawing, the pivot, a resize handle's pointer): each axis onto the x or y of a corner or centre of a node's bounds or a row end, else onto the grid.
- **Moves** (the select tool): each axis of the moved selection's bounds (min, centre, max) onto the min, centre or max of another node's bounds; equal spacing: along an axis, the nearest sibling on one side that overlaps the moved box across the axis, and the sibling beyond it overlapping that one, give a gap, and the moved box takes the same gap to the first; the grid: the box's top-left corner onto the grid. Row alignment, for a single straight row block: its front row onto a row line of a sibling straight block with the same rotation (a whole number of that block's row pitches from its front row, along its own y axis). When that axis is a world axis it competes on it as the best kind; when the blocks are turned, the row alignment alone decides the move.
- **Angles**: a rotation onto a multiple of 15° of the angle turned, and for a single row block onto the angle that turns its front (−y) towards the focal point of the floor shown as seen from the pivot, which wins over the 15° steps; the reach is the threshold at the pointer's distance from the pivot. A line's end goes onto edges and centres first, then onto a 15° direction from its start (with the same reach), then onto the grid, which, finer than the threshold at most zooms, would otherwise always win. Shift still rounds to 15° itself.

**Guides.** The overlay's `guides`: a `line` (magenta) at the coordinate snapped to, spanning the target and the moved box or point; two `spacing` markers, each a gap with its distance; a `row` extension (dashed), the neighbour's row line across both blocks; an `angle` arc about the pivot (or a line's start) from the start direction to the snapped one. The grid shows as dots drawn by the canvas from the step. User guides (DSN-031) are another target kind for points and moves, beside edges and centres.

The index (`SpatialIndex`) caches each node's bounds in world space and in its own space, from its expanded seats for row blocks and rows (the expansion is cached per node and redone only when the node's content revision changes) and from its fields otherwise, and is brought up to date from `Document::changes_since`; only a change of seat size or alphabet overflow recomputes every node. A chart is a few hundred to a few thousand nodes, so queries scan cached bounds:

- `hit`: the topmost node at a point, among one level's children, that the caller's filter accepts (§5.1 picking), within a tolerance. A row block or row is hit within half the diagonal between neighbouring seats of a present seat, so the space between its seats and rows is on it and an aisle is not; a section through a child or its explicit outline's box; any other node inside its box in its own space, so a turned node is not hit in the corners of its world bounds.
- `select`: the nodes at one level whose world bounds a box contains or, with Alt, that it touches (as in §5.1).
- `anchors`: bounds corners, centres and row ends of nodes touching a region, for snapping.
- `seat_at`: the seat position nearest a point, for rows and seat-edit mode.

A row block or row whose labels do not expand (halfway through an edit, or with more rows than letters label in 64 bytes) is placed with plain numbers, so it can still be picked. An SVG `path` shape's box is that of its end and control points, reflected ones included: a Bézier curve lies in the hull of its control points, so the box holds what the compiler draws. Data that does not parse keeps the points before the error, so the shape can still be selected and fixed.

### 5.3 Row blocks

Inspector fields:

| Field | Notes |
|---|---|
| Rows, seats per row | Editing "seats per row" applies to all rows; individual rows can differ |
| Seat pitch, row pitch | In cu |
| Curve | None, arc (radius), bulge (amount). A handle on the middle of the front row adjusts it by dragging |
| Alignment | Left, centre, right, for rows of different length |
| Stagger | Off or half a pitch |
| Skew | Degrees |
| Seat size and shape | Defaults from the chart; overridable per block |
| Category | For the whole block |
| Row labels, seat labels | §6 |

Row-level edits (after double-click): add or remove seats at either end; shift a row sideways; insert an aisle (splits the row into segments by removing a position); override a row's label; delete a row.

Seat-level edits (seat-edit mode): remove or restore a seat; set flags; set category; override a label; nudge position.

### 5.4 Tables, booths, areas

| Node | Inspector fields |
|---|---|
| Table | Shape, size, seats, seat spacing, "book as" (seats or whole table), label, seat numbering start and direction, category |
| Booth | Size, label, category |
| Area | Outline (editable vertices), label, capacity, category |

### 5.5 Sections

- A section is created around existing nodes or drawn empty (the section tool, §4, or `Session::group(ids, outline)` with top-level nodes, sections refused, and an optional explicit outline in world points). It goes where the lowest of those nodes was among the top-level nodes (draw order kept), or at the end; its `origin` is the centre of its explicit outline's box, or of its nodes' bounds, rotation 0; its label is `Section N` for the smallest N that no section's label uses (W109 warns on unlabelled sections); its `category` is the active category; `priority` 1. Creating it is one gesture of `createNode` then one `placeNode` per node, one undo entry; at most 65,535 sections, 4,096 points per outline and 262,144 in all outlines of the chart.
- **Reparenting keeps the canvas still.** A child's `origin` and `rotation` are in its section's space; `placeNode` (§2.2) rewrites them whenever a node changes parent, so grouping, dissolving and the layers panel's moves between sections never move a seat. `Session::dissolve(id)` puts a section's children at the top level where it was, in order, and deletes it: one undo entry. A child keeps its `floor` field; one naming another floor than its section's is E007.
- Its outline is automatic by default and follows its contents. It can be converted to an editable polygon: `Session::convert_outline(id)` sets `outline` to explicit points equal to the automatic outline as the compiler computes it now (`cavea_compiler::auto_outline` on the section's visible blocks' and rows' present seats in its space, with the compiler's padding), as one `setProperty outline`, whose inverse restores `"auto"`. The seats come from the index's cached expansion carried by each child's transform; for unturned children the points are the compiler's exactly, so the compiled chart is the same byte for byte. A section without seats, or with an explicit outline, is refused.
- Inspector: label, default category, entrance text, best-available priority, label position.
- Dragging nodes in the layers panel moves them between sections.

### 5.6 Floors

- The floor switcher lists floors; add, rename, reorder, delete.
- Other floors can be shown dimmed underneath as a reference.
- Each floor has its own focal point.

## 6. Numbering

One panel sets the labels for the selected blocks, with the result previewed on the canvas before it is applied.

**Rows**

| Control | Options |
|---|---|
| Scheme | A, B, C · a, b, c · 1, 2, 3 · I, II, III · custom list |
| Start at | Any label in the scheme |
| Skip | Chosen letters, for example I and O |
| Direction | Front to back, back to front |
| After Z | AA, BB, CC or AA, AB, AC |
| Prefix, suffix | Free text |

**Seats**

| Control | Options |
|---|---|
| Scheme | 1, 2, 3 · A, B, C · odd only · even only · custom list |
| Start at, step | |
| Direction | Left to right, right to left, centre outward (odd one side, even the other), serpentine |
| Continuous | Carry on from the previous row |
| Across blocks | Continue numbering through the selected blocks in a chosen order |
| Prefix, suffix | |

Left and right are defined facing the focal point, and the panel says so.

The label template (`{section}-{row}-{seat}`) is a chart setting. A live sample shows the full label of the hovered seat.

## 7. Categories

- The Categories panel lists categories with colour, label, key, an "accessible" marker and the number of objects using each.
- Add, rename, recolour, merge, delete (deleting requires reassigning its objects).
- The colour picker shows a contrast warning when two categories are hard to tell apart or when a colour is weak against the canvas background.
- Prices are not edited here. They belong to the customer's system.

## 8. Keyboard shortcuts

| Action | Shortcut |
|---|---|
| Undo, redo | Ctrl+Z, Ctrl+Shift+Z |
| Copy, cut, paste, duplicate | Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+D |
| Delete | Delete or Backspace |
| Select all, deselect | Ctrl+A, Escape |
| Group, ungroup | Ctrl+G, Ctrl+Shift+G |
| Lock, hide | Ctrl+L, Ctrl+H |
| Zoom in, out, fit, to selection | Ctrl+=, Ctrl+−, Ctrl+0, Ctrl+2 |
| Pan | Space + drag, or middle mouse |
| Nudge | Arrows; Shift+arrows for 10× |
| Toggle snapping, grid, rulers | Ctrl+', Ctrl+;, Ctrl+R |
| Save now | Ctrl+S |
| Command palette | Ctrl+K |
| Tool keys | As in §4 |

On macOS, Cmd replaces Ctrl. The command palette lists every action with its shortcut.

## 9. Undo, redo and saving

- Every command pushes its inverse onto the undo stack. Redo is cleared by a new command.
- A drag produces one undo entry, however many intermediate states it passed through.
- Property scrubbing (dragging a number field) coalesces into one entry.
- The stack holds the last 500 entries for the session.
- **Entries.** An entry is the list of internal operations that restores the state before it. Undo applies them, and they return the operations that redo it, so neither copies the document; an entry holds only what its command changed (a deleted block's data, a moved node's old transform). Undo or redo that fails part-way puts back what it applied and keeps the entry.
- **Gestures.** A tool opens a gesture (pointer down on the canvas or on a number field), applies commands that change the document at once, and closes it as one entry. Within a gesture only the first inverse recorded for each node transform, node property, chart property, category list, floor list, seat override and `nextOid` is kept: it alone restores the start, so a drag of any length holds one old transform per node. Escape cancels the gesture: the document returns to its start, no entry is left and redo is kept. Undo and redo are refused while a gesture is open.
- **`nextOid` across undo.** Undo restores `nextOid` with everything else, so applying and undoing returns the document byte for byte (`13-testing-benchmarks.md` §9). Ids are still never handed out twice (`02-domain-model.md` §3): new ids come from a counter in the editor that starts above every id in the loaded document and never goes back, undo included, and the saved draft writes `nextOid` as the larger of the document's value and that counter, so a draft saved after an undone creation still reserves its ids. Redo re-inserts the same nodes with the same ids. A command that fails hands its ids back, since nothing holds them. A cancelled gesture does not: its ids may have been seen (in `Applied`, or by a reader of the document while it was open), so like an undone creation they stay reserved, and the saved draft's `nextOid` covers them.
- **Autosave:** two seconds after the last change, the draft is sent with `If-Match`. The top bar shows "Saving…", "Saved", or "Offline — changes kept locally".
- **Local safety net:** unsaved changes are also written to IndexedDB and offered for recovery after a crash or a closed tab.
- **Concurrency:** one editor at a time per chart in v1. A second person opening the chart sees who is editing and may take over; the first is switched to read-only with their unsaved work kept locally. Real-time co-editing is not planned for v1.

## 10. Validation

- Validation runs continuously in the edit core on the changed nodes, with the rules of `03-chart-format.md` §3.2.
- The Validation panel lists errors and warnings. Clicking one selects and frames the offending node.
- Offending objects are marked on the canvas.
- Publish is disabled while errors exist.

## 11. Preview and publish

**Preview** opens the real buyer renderer on the live-compiled chart, with a device-width switcher (phone, tablet, desktop) and sample states (empty house, half sold, nearly sold out).

**Publish:**

1. Compile on the server.
2. Show the **diff** against the previous version: objects added, removed, relabelled, moved between categories.
3. If the chart has events with non-free objects, show the **impact**: which bookings would lose their object in an upgrade. Removal or relabelling of such objects needs explicit confirmation by typing the chart name.
4. Create the version, render and upload the thumbnail.
5. Offer to upgrade events that use the previous version, listing each with a safe or conflict status.

**Safe mode** is on by default for a chart that has events with non-free objects. It warns at edit time when a change touches such an object, so the user is not surprised at publish.

## 12. Import and export

| Function | Behaviour | Priority |
|---|---|---|
| Export JSON | Downloads the chart document | P0 |
| Import JSON | Creates a chart or replaces the draft | P0 |
| Reference image | Place a floor plan, set its opacity, lock it, and draw over it | P0 |
| Scale calibration | Draw a line over a known length on the reference image to set its scale | P1 |
| CSV import | Columns for section, row, seat, x, y, category; groups points into rows | P1 |
| seats.io import | Converts their chart JSON into a chart document | P1 |
| SVG and DXF import | Shapes become decoration; repeated small shapes can be converted to seats | P2 |
| AI import | `10-ai-import.md` | P2 |
| Export SVG | For printed seating plans | P2 |

## 13. Embedding

- The designer is served at `/embed/designer` for iframes.
- The customer's backend requests an embed token (`POST /v1/embed-tokens`) scoped to a workspace and a chart, with permissions: `edit`, `publish`, `manage_categories`, `import`.
- `@cavea/designer-embed` wraps the iframe:

```ts
const designer = mountDesigner(el, {
  baseUrl: 'https://tickets.example.com/cavea',
  token,                                  // embed token
  chart: 'royal',
  theme: { accent: '#0F766E', logo: null },
  features: { publish: true, import: false, floors: true },
  onSaved: (e) => {},
  onPublished: (e) => {},                 // { chart, version, hash, seats }
  onExit: () => {},
});
```

- Communication is by `postMessage` with an origin check on both sides.
- White-label: accent colour, hidden branding, hidden menu items, custom exit action.

## 14. Performance

| Situation | Approach |
|---|---|
| Dragging a large selection | Selected chunks are drawn with a per-chunk transform; positions are baked on release (`04-renderer.md` §25) |
| Changing a block parameter | Only that block is re-expanded and re-uploaded |
| Inspector updates during a drag | View models are throttled to one per frame |
| Layers panel on a large chart | Virtualised list, fed page by page |
| Validation | Incremental per changed node; the full pass runs in idle time |
| Autosave of a large document | Serialised in the edit core; compressed before upload |

Targets are in `01-architecture.md` §5.

## 15. Accessibility of the designer

- All chrome is keyboard-operable with visible focus, labelled controls and logical tab order.
- Every canvas action has an equivalent in the inspector, the layers panel or the command palette.
- Drawing by pointer has a non-pointer alternative for the main objects: "Insert row block…" opens a form for rows, seats, pitch and position.
- The designer does not aim to be fully usable by screen-reader users in v1; the buyer renderer does (`11-accessibility.md`).

## 16. Empty and first-run states

- **New chart:** a centred prompt with three starting points: draw from scratch, start from a template (theatre, arena bowl, banquet), or place a reference image.
- **First run:** a five-step coach-mark tour: draw a block, curve it, number it, assign a category, publish.
- **Templates** are ordinary chart documents shipped with the server.
