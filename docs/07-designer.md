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
- Alt disables snapping temporarily.
- Numeric entry: after starting a drag, typing a number and Enter sets the exact value shown in the live readout (rows, seats, radius).

## 5. Editing

### 5.1 Selection and transform

- Click selects the top-level node (block, table, shape). Double-click enters it: first to rows, then to seats (**seat-edit mode**). Escape steps back out.
- Drag box selects what it fully contains; with Alt held, what it touches.
- Move: drag, or arrow keys (1 cu; with Shift, 10 cu).
- Rotate: a handle outside the selection box; snaps to 15° with Shift; the pivot is draggable.
- Resize: on a row block, dragging a side handle adds or removes rows or seats; with Ctrl held it changes the pitch instead. On shapes it scales.
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

- A section is created around existing nodes or drawn empty.
- Its outline is automatic by default and follows its contents. It can be converted to an editable polygon.
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
