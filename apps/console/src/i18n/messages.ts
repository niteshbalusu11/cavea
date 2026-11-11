// The English message catalogue (09 §8: every string in a catalogue).
// Components show `msg(id)`, never a literal. Each group keyed by values of
// the edit core's types is typed against those types, and messages.test.ts
// fails when the generated types (or the compiler's codes) gain a value
// that has no message here.
import type {
  AlphaOverflow,
  Align,
  CommandError,
  ImageRole,
  ImportError,
  Layer,
  NodeKind,
  RowLabelDisplay,
  SeatShape,
  ShapeKind,
  ToolId,
  Unit,
} from "@cavea/designer-core";

/** Prefixes each key of a group: `group("kind", {row: …})` → `{"kind.row": …}`. */
function group<P extends string, K extends string>(
  prefix: P,
  entries: Record<K, string>,
): Record<`${P}.${K}`, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries<string>(entries)) out[`${prefix}.${k}`] = v;
  return out as Record<`${P}.${K}`, string>;
}

const kinds: Record<NodeKind, string> = {
  section: "Section",
  rowBlock: "Row block",
  row: "Row",
  table: "Table",
  booth: "Booth",
  area: "Area",
  shape: "Shape",
  text: "Text",
  icon: "Icon",
  image: "Image",
};

/** Plural, for "3 row blocks". */
const kindsPlural: Record<NodeKind, string> = {
  section: "sections",
  rowBlock: "row blocks",
  row: "rows",
  table: "tables",
  booth: "booths",
  area: "areas",
  shape: "shapes",
  text: "texts",
  icon: "icons",
  image: "images",
};

const tools: Record<ToolId, string> = {
  select: "Select",
  hand: "Hand",
  rowBlock: "Row block",
  arcBlock: "Arc block",
  row: "Single row",
  rect: "Rectangle",
  ellipse: "Ellipse",
  polygon: "Polygon",
  line: "Line",
  text: "Text",
  section: "Section",
  brush: "Category brush",
  focal: "Focal point",
};

/** Inspector fields (`Field`'s names), which also name inspector undo entries. */
export const fields = {
  name: "Name",
  x: "X",
  y: "Y",
  rotation: "Rotation",
  locked: "Locked",
  hidden: "Hidden",
  floor: "Floor",
  category: "Category",
  label: "Label",
  rows: "Rows",
  seats: "Seats per row",
  seatsInRow: "Seats",
  seatPitch: "Seat pitch",
  rowPitch: "Row pitch",
  curve: "Curve",
  align: "Alignment",
  stagger: "Stagger",
  skew: "Skew",
  priority: "Priority",
  entrance: "Entrance",
  width: "Width",
  height: "Height",
  cornerRadius: "Corner radius",
  fill: "Fill",
  stroke: "Stroke",
  strokeWidth: "Stroke width",
  opacity: "Opacity",
  layer: "Layer",
  text: "Text",
  textSize: "Size",
  weight: "Weight",
  color: "Colour",
  textAlign: "Alignment",
  minZoom: "Minimum zoom",
  imageRole: "Role",
  chartName: "Chart name",
  seatSize: "Seat size",
  seatShape: "Seat shape",
  labelTemplate: "Seat label",
  tableLabelTemplate: "Table seat label",
  rowLabelDisplay: "Row labels",
  alphaOverflow: "After Z",
  // Property fields that are not inspector fields.
  outline: "Outline",
  rowLabels: "Row labels",
  seatLabels: "Seat labels",
  decoration: "Edit",
  // Inspector-only names.
  curveRadius: "Radius",
  curveAmount: "Amount",
  autoOutline: "Outline",
  kind: "Shape",
} as const;

/** Undo entries named by a command's `op` or a session call (07 §2.3). */
const history = {
  createNode: "Create",
  deleteNodes: "Delete",
  moveNodes: "Move",
  transformNodes: "Transform",
  placeNode: "Arrange",
  setProperty: "Edit",
  setChart: "Chart settings",
  chartSettings: "Chart settings",
  addCategory: "Add category",
  updateCategory: "Edit category",
  removeCategory: "Delete category",
  moveCategory: "Reorder categories",
  addFloor: "Add floor",
  updateFloor: "Edit floor",
  removeFloor: "Delete floor",
  addRows: "Add rows",
  removeRows: "Remove rows",
  addSeats: "Add seats",
  removeSeats: "Remove seats",
  resizeRows: "Resize rows",
  removeSeat: "Remove seat",
  restoreSeat: "Restore seat",
  setSeatOverride: "Edit seat",
  editSeats: "Edit seats",
  shiftRows: "Shift rows",
  setRowLabel: "Row label",
  insertGap: "Open aisle",
  flipNodes: "Flip",
  composite: "Edit",
  moveLayers: "Move layers",
  gesture: "Edit",
};

const units: Record<Unit, string> = {
  cu: "cu",
  degrees: "°",
  count: "",
  fraction: "",
  zoom: "×",
  number: "",
};

const commandErrors: Record<CommandError["code"], string> = {
  missingNode: "That object no longer exists.",
  notASection: "Objects can only be moved into a section.",
  notARowBlock: "That needs a row block.",
  cycle: "A section cannot be moved into itself.",
  placement: "Sections stay at the top level: they cannot go inside another section.",
  indexOutOfRange: "That position is outside the list.",
  notApplicable: "{field} does not apply to everything selected.",
  invalidValue: "That value is not allowed for {field}.",
  limit: "That would exceed the chart's limit on {what}.",
  idsExhausted: "The chart has run out of object ids.",
  duplicateId: "Two objects would share an id.",
  missingRow: "That row no longer exists.",
  invalidRow: "That row is damaged and cannot be edited.",
  missingSeat: "That seat no longer exists.",
  seatState: "That seat is already in that state.",
  duplicateCategory: "A category with the key “{key}” already exists.",
  missingCategory: "There is no category “{key}”.",
  categoryInUse: "“{key}” is still used {count} times: choose a category to replace it.",
  missingFloor: "That floor no longer exists.",
  gestureOpen: "Finish the current drag first.",
  noGesture: "There is nothing to finish.",
  internal: "The editor hit an internal inconsistency; nothing was changed.",
};

const importErrors: Record<ImportError["code"], string> = {
  tooLarge: "The file is {bytes}; a chart can be at most {limit}.",
  json: "This is not a chart document: line {line}, column {column}: {message}",
  schema: "The file uses chart schema {schema}, which this designer does not read.",
  migration: "The file's schema {from} could not be upgraded: {problem}",
  nulEscape: "The file contains a \\u0000 escape, which cannot be stored.",
  duplicateId: "Two objects in the file share the id {id}: fix the file and try again.",
  limit: "The file has {count} {what}; a chart can hold at most {limit}.",
  seatsPerRow: "A row of object {node} has {seats} seats; a row can hold at most {limit}.",
};

/** Short titles for the compiler's finding codes (03 §3.2). */
const validation = {
  E001: "Duplicate label",
  E002: "Seat without a category",
  E003: "Empty label",
  E004: "Area without a capacity",
  E005: "Invalid geometry",
  E006: "Limit exceeded",
  E007: "Unknown reference",
  E008: "Duplicate category key",
  E009: "Duplicate id",
  E010: "Unsupported schema",
  unsupported: "Not supported yet",
  W101: "Overlapping seats",
  W102: "No focal point",
  W103: "Single-seat row",
  W104: "Seat outside its section",
  W105: "Unused category",
  W106: "No accessible seats",
  W107: "Colours hard to tell apart",
  W108: "Text too small",
  W109: "Section without a label",
  W110: "Chart very large",
};

const enums = {
  ...group("seatShape", { circle: "Circle", rounded: "Rounded", chair: "Chair" } satisfies Record<
    SeatShape,
    string
  >),
  ...group("rowLabelDisplay", {
    none: "None",
    start: "Start",
    end: "End",
    both: "Both ends",
  } satisfies Record<RowLabelDisplay, string>),
  ...group("alphaOverflow", {
    double: "AA, BB, CC",
    spreadsheet: "AA, AB, AC",
  } satisfies Record<AlphaOverflow, string>),
  ...group("align", { left: "Left", center: "Centre", right: "Right" } satisfies Record<
    Align,
    string
  >),
  ...group("layer", { below: "Below seats", above: "Above seats" } satisfies Record<Layer, string>),
  ...group("imageRole", { reference: "Reference", background: "Background" } satisfies Record<
    ImageRole,
    string
  >),
  ...group("shapeKind", {
    rect: "Rectangle",
    ellipse: "Ellipse",
    polygon: "Polygon",
    line: "Line",
    path: "Path",
  } satisfies Record<ShapeKind, string>),
  ...group("curve", { none: "Straight", arc: "Arc", bulge: "Bulge" }),
};

const ui = {
  "app.name": "Cavea",
  "nav.label": "Console",
  "nav.charts": "Charts",
  "nav.events": "Events",
  "nav.reports": "Reports",
  "nav.developers": "Developers",
  "nav.settings": "Settings",
  "nav.theme": "Theme",
  "theme.system": "System",
  "theme.light": "Light",
  "theme.dark": "Dark",
  "notBuilt.title": "{section} is not built yet",
  "notBuilt.body": "This screen arrives with a later slice of the console.",
  "charts.title": "Charts",
  "charts.body":
    "The chart list arrives with the server's console endpoints. Until then, open a chart document from a file in the designer.",
  "charts.openDesigner": "Open the designer",
  "notFound.title": "Page not found",
  "notFound.back": "Go to charts",

  "designer.loading": "Loading the designer…",
  "designer.loadFailed": "The designer could not start: {message}",
  "designer.narrow": "The designer needs a window at least {width} pixels wide.",
  "designer.narrowBody": "Widen the window, or zoom the page out.",
  "designer.empty": "No chart is open",
  "designer.untitled": "Untitled chart",
  "designer.emptyBody": "Open a chart document, or start an empty chart.",
  "designer.dropHint": "Drop a chart document to open it",

  "menu.label": "Menu",
  "menu.new": "New empty chart",
  "menu.open": "Open chart file…",
  "menu.export": "Export chart file",
  "menu.theme": "Theme",
  "menu.console": "Back to console",
  "topbar.chartName": "Chart name",
  "topbar.undoWhat": "Undo {what}",
  "topbar.redoWhat": "Redo {what}",
  "topbar.nothingToUndo": "Nothing to undo",
  "topbar.nothingToRedo": "Nothing to redo",
  "topbar.saved": "Exported {time}",
  "topbar.unsaved": "Unsaved changes",
  "topbar.floor": "Floor",
  "topbar.singleFloor": "Single floor",
  "topbar.preview": "Preview",
  "topbar.publish": "Publish",
  "topbar.previewPending": "Preview arrives with a later slice.",
  "topbar.publishPending": "Publishing needs the server, which arrives with a later slice.",
  "topbar.publishBlocked": "Fix the errors in the Validation panel to publish.",
  "topbar.publishChecking": "Checking the chart…",

  "rail.label": "Tools",
  "canvas.label": "Canvas",
  "canvas.placeholder": "The canvas arrives with DSN-004.",
  "canvas.summary": "{seats} seats · {nodes} objects",
  "canvas.camera": "Centre {x}, {y} · zoom {zoom}",

  "status.seats": "{n} seats",
  "status.nodes": "{n} objects",
  "status.selected": "{n} selected",
  "status.problems": "{errors} errors · {warnings} warnings",
  "status.noProblems": "No problems",
  "status.checking": "Checking…",
  "status.snap": "Snap",
  "status.grid": "Grid",
  "status.zoomIn": "Zoom in",
  "status.zoomOut": "Zoom out",
  "status.zoomFit": "Zoom to fit",
  "status.cursor": "x {x}  y {y}",

  "panel.inspector": "Inspector",
  "panel.panels": "Panels",
  "panel.layers": "Layers",
  "panel.categories": "Categories",
  "panel.validation": "Validation",
  "panel.collapse": "Collapse {panel}",
  "panel.expand": "Expand {panel}",
  "panel.resize": "Resize {panel}",

  "inspector.chart": "Chart",
  "inspector.settings": "Chart settings",
  "inspector.selected": "{n} selected",
  "inspector.many": "{n} {kinds}",
  "inspector.mixed": "Mixed",
  "inspector.inherit": "Inherit",
  "inspector.position": "Position",
  "inspector.numberingPending":
    "Row and seat labels are set in the numbering panel, which is not built yet.",
  "inspector.level.rows": "Rows of a row block",
  "inspector.level.seats": "Seats",
  "inspector.levelHint": "Press Escape to step out to the objects.",
  "inspector.readOnly": "Set by the row's path",
  "inspector.outlineAuto": "Automatic",
  "inspector.outlineExplicit": "Explicit points",
  "inspector.convertOutline": "Make editable",
  "inspector.range": "Between {min} and {max}",
  "inspector.notNumber": "Enter a number",

  "layers.label": "Layers",
  "layers.empty": "This floor has no objects.",
  "layers.unnamed": "{kind} {id}",
  "layers.lock": "Lock",
  "layers.unlock": "Unlock",
  "layers.hide": "Hide",
  "layers.show": "Show",
  "layers.forward": "Bring forward",
  "layers.backward": "Send backward",
  "layers.front": "Bring to front",
  "layers.back": "Send to back",
  "layers.moveUp": "Move up",
  "layers.moveDown": "Move down",
  "layers.duplicate": "Duplicate",
  "layers.delete": "Delete",
  "layers.group": "Group into section",
  "layers.dissolve": "Dissolve section",

  "categories.label": "Categories",
  "categories.add": "Add category",
  "categories.count": "{n} categories",
  "categories.newLabel": "New category",
  "categories.key": "Key",
  "categories.labelField": "Label",
  "categories.colour": "Colour",
  "categories.hex": "Hex colour",
  "categories.accessible": "Accessible",
  "categories.accessibleHint": "Wheelchair or accessible seating",
  "categories.objects": "{n} objects",
  "categories.active": "Active",
  "categories.makeActive": "Make active",
  "categories.moveUp": "Move up",
  "categories.moveDown": "Move down",
  "categories.merge": "Merge into…",
  "categories.delete": "Delete…",
  "categories.edit": "Edit {label}",
  "categories.weak": "Weak against the canvas: {light}:1 light, {dark}:1 dark (3:1 needed).",
  "categories.similar": "Hard to tell apart from {others}.",
  "categories.badHex": "Use a colour like #2563EB.",
  "categories.empty": "No categories yet. Add one so seats can be sold at a price.",
  "categories.deleteTitle": "Delete “{label}”",
  "categories.deleteBody": "{n} objects use “{label}”. Choose the category they move to.",
  "categories.deleteUnused": "No object uses “{label}”.",
  "categories.replacement": "Move them to",
  "categories.mergeTitle": "Merge “{label}” into another category",
  "categories.mergeBody":
    "Its {n} objects move to the category you choose, and “{label}” is deleted.",
  "categories.mergeInto": "Merge into",

  "validation.label": "Validation",
  "validation.errors": "Errors",
  "validation.warnings": "Warnings",
  "validation.none": "No problems found.",
  "validation.checking": "Checking the whole chart…",
  "validation.stale": "From the last full check",
  "validation.encode": "The chart cannot be compiled: {message}",

  "action.ok": "OK",
  "action.delete": "Delete",
  "action.merge": "Merge",
  "action.cancel": "Cancel",
  "action.close": "Close",
  "action.reload": "Reload",
  "action.discard": "Discard and open",

  "confirm.replaceTitle": "Replace the open chart?",
  "confirm.replaceBody":
    "“{name}” has changes that have not been exported. Opening another chart discards them, with their undo history.",
  "import.failed": "Could not open {file}",
  "import.notJson": "Only chart document files (.json) can be opened.",
  "import.unreadable":
    "The file could not be read. It may have moved, or the browser lacks permission to read it.",
  "export.done": "Exported {file}",
  "clipboard.empty": "The clipboard holds nothing to paste.",

  "panic.title": "The editor stopped",
  "panic.body":
    "The editor hit a bug and stopped. Changes since your last export cannot be trusted and are not offered for export. Reload the page to continue from your last exported file.",
  "panic.detail": "Details for a bug report: {message}",

  "error.arguments": "The editor refused that call: {message}",
  "error.generic": "Something went wrong: {message}",

  "shortcut.undo": "Undo",
  "shortcut.redo": "Redo",
  "shortcut.copy": "Copy",
  "shortcut.cut": "Cut",
  "shortcut.paste": "Paste",
  "shortcut.duplicate": "Duplicate",
  "shortcut.delete": "Delete",
  "shortcut.selectAll": "Select all",
  "shortcut.deselect": "Deselect or step out",
  "shortcut.stepIn": "Step in",
  "shortcut.group": "Group",
  "shortcut.ungroup": "Ungroup",
  "shortcut.lock": "Lock or unlock",
  "shortcut.hide": "Hide or show",
  "shortcut.zoomIn": "Zoom in",
  "shortcut.zoomOut": "Zoom out",
  "shortcut.zoomFit": "Zoom to fit",
  "shortcut.zoomSelection": "Zoom to selection",
  "shortcut.nudge": "Nudge",
  "shortcut.snap": "Toggle snapping",
  "shortcut.grid": "Toggle grid",
  "shortcut.rulers": "Toggle rulers",
  "shortcut.save": "Save now",
  "shortcut.palette": "Command palette",
  "shortcut.help": "Keyboard shortcuts",
  "shortcut.table": "Table tool",
  "shortcut.booth": "Booth tool",
  "shortcut.area": "Area tool",
  "shortcut.image": "Image tool",
  "shortcut.measure": "Measure tool",
};

export const catalogue = {
  ...ui,
  ...group("kind", kinds),
  ...group("kinds", kindsPlural),
  ...group("tool", tools),
  ...group("field", fields),
  ...group("history", history),
  ...group("unit", units),
  ...group("commandError", commandErrors),
  ...group("importError", importErrors),
  ...group("validation", validation),
  ...enums,
};

export type MessageId = keyof typeof catalogue;
export type Params = Record<string, string | number>;

/** The message `id` with `{name}` placeholders filled; numbers through `Intl`. */
export function msg(id: MessageId, params?: Params): string {
  const text: string = catalogue[id];
  if (params === undefined) return text;
  return text.replace(/\{(\w+)\}/g, (all, name: string) => {
    const v = params[name];
    return v === undefined ? all : typeof v === "number" ? formatNumber(v) : v;
  });
}

/** A message whose id is built from a value, or `null` when there is none. */
export function msgOf(prefix: string, key: string, params?: Params): string | null {
  const id = `${prefix}.${key}`;
  return id in catalogue ? msg(id as MessageId, params) : null;
}

/** The name an undo entry's key shows (07 §9). */
export function historyName(key: string): string {
  return msgOf("history", key) ?? msgOf("field", key) ?? msg("history.gesture");
}

const LOCALE = "en";
const numbers = new Intl.NumberFormat(LOCALE, { maximumFractionDigits: 2 });
const times = new Intl.DateTimeFormat(LOCALE, { hour: "2-digit", minute: "2-digit" });
const bytes = new Intl.NumberFormat(LOCALE, {
  style: "unit",
  unit: "megabyte",
  maximumFractionDigits: 1,
});

export const formatNumber = (n: number): string => numbers.format(n);
export const formatTime = (d: Date): string => times.format(d);
export const formatBytes = (n: number): string => bytes.format(n / 1_000_000);
