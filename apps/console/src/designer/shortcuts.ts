// The designer's keyboard shortcuts (07 §4, §8) as one table: it binds the
// keys, and it feeds the tooltips and, later, the command palette and the
// `?` help (DSN-009). On macOS, Cmd replaces Ctrl. An action that does not
// exist yet is in the table as `pending`, with the task that brings it,
// and is bound to nothing.
import type { ToolId } from "@cavea/designer-core";
import type { MessageId } from "../i18n/messages.ts";

export interface Chord {
  /** `KeyboardEvent.key`, lower case for a printable key. */
  key: string;
  /** Ctrl, or Cmd on macOS. */
  mod?: boolean;
  /** Required state of Shift; `"any"` accepts either. */
  shift?: boolean | "any";
  alt?: boolean;
}

/**
 * How a key reaches the edit core: `call`, an action of the shell calling a
 * session method; `tool`, the tool rail; `input`, forwarded to the active
 * tool through `Session.input` (it steps in and out, deletes, nudges,
 * cancels and takes numeric entry, by tool and level: 07 §4, §5.1).
 */
export type Route = "call" | "tool" | "input";

export interface Shortcut {
  id: string;
  label: MessageId;
  chords: Chord[];
  route: Route;
  /** Fires while a text field has focus (only what the platform does not
   * do in a field itself). */
  inFields?: true;
  /** The task that brings the action; bound to nothing until then. */
  pending?: string;
}

const tool = (id: ToolId, key: string, shift = false): Shortcut => ({
  id: `tool.${id}`,
  label: `tool.${id}`,
  chords: [{ key, shift }],
  route: "tool",
});
const later = (id: string, label: MessageId, chords: Chord[], task: string): Shortcut => ({
  id,
  label,
  chords,
  route: "call",
  pending: task,
});
const arrows = ["arrowleft", "arrowright", "arrowup", "arrowdown"];

export const SHORTCUTS: readonly Shortcut[] = [
  { id: "undo", label: "shortcut.undo", chords: [{ key: "z", mod: true }], route: "call" },
  {
    id: "redo",
    label: "shortcut.redo",
    chords: [{ key: "z", mod: true, shift: true }],
    route: "call",
  },
  { id: "copy", label: "shortcut.copy", chords: [{ key: "c", mod: true }], route: "call" },
  { id: "cut", label: "shortcut.cut", chords: [{ key: "x", mod: true }], route: "call" },
  { id: "paste", label: "shortcut.paste", chords: [{ key: "v", mod: true }], route: "call" },
  {
    id: "duplicate",
    label: "shortcut.duplicate",
    chords: [{ key: "d", mod: true }],
    route: "call",
  },
  {
    id: "selectAll",
    label: "shortcut.selectAll",
    chords: [{ key: "a", mod: true }],
    route: "call",
  },
  { id: "lock", label: "shortcut.lock", chords: [{ key: "l", mod: true }], route: "call" },
  { id: "hide", label: "shortcut.hide", chords: [{ key: "h", mod: true }], route: "call" },
  {
    id: "zoomIn",
    label: "shortcut.zoomIn",
    chords: [
      { key: "=", mod: true, shift: "any" },
      { key: "+", mod: true, shift: "any" },
    ],
    route: "call",
  },
  { id: "zoomOut", label: "shortcut.zoomOut", chords: [{ key: "-", mod: true }], route: "call" },
  { id: "zoomFit", label: "shortcut.zoomFit", chords: [{ key: "0", mod: true }], route: "call" },
  {
    id: "zoomSelection",
    label: "shortcut.zoomSelection",
    chords: [{ key: "2", mod: true }],
    route: "call",
  },
  { id: "snap", label: "shortcut.snap", chords: [{ key: "'", mod: true }], route: "call" },
  { id: "grid", label: "shortcut.grid", chords: [{ key: ";", mod: true }], route: "call" },
  {
    id: "save",
    label: "shortcut.save",
    chords: [{ key: "s", mod: true }],
    route: "call",
    inFields: true,
  },
  {
    id: "delete",
    label: "shortcut.delete",
    chords: [{ key: "delete" }, { key: "backspace" }],
    route: "input",
  },
  { id: "deselect", label: "shortcut.deselect", chords: [{ key: "escape" }], route: "input" },
  { id: "stepIn", label: "shortcut.stepIn", chords: [{ key: "enter" }], route: "input" },
  {
    id: "nudge",
    label: "shortcut.nudge",
    chords: arrows.map((key) => ({ key, shift: "any" })),
    route: "input",
  },
  tool("select", "v"),
  tool("hand", "h"),
  tool("rowBlock", "r"),
  tool("arcBlock", "a"),
  tool("row", "r", true),
  tool("rect", "u"),
  tool("text", "x"),
  tool("section", "s"),
  tool("brush", "c"),
  tool("focal", "f"),
  later(
    "group",
    "shortcut.group",
    [{ key: "g", mod: true }],
    "07 §5.1: decoration groups need schema 2",
  ),
  later(
    "ungroup",
    "shortcut.ungroup",
    [{ key: "g", mod: true, shift: true }],
    "07 §5.1: decoration groups need schema 2",
  ),
  later("rulers", "shortcut.rulers", [{ key: "r", mod: true }], "DSN-031"),
  later("palette", "shortcut.palette", [{ key: "k", mod: true }], "DSN-009"),
  later("help", "shortcut.help", [{ key: "?", shift: "any" }], "DSN-009"),
  later("tool.table", "shortcut.table", [{ key: "t" }], "DSN-050"),
  later("tool.booth", "shortcut.booth", [{ key: "b" }], "DSN-051"),
  later("tool.area", "shortcut.area", [{ key: "g" }], "DSN-052"),
  later("tool.image", "shortcut.image", [{ key: "i" }], "DSN-020"),
  later("tool.measure", "shortcut.measure", [{ key: "m" }], "DSN-054"),
];

/** The layers tree's own keys for moving a row (07 §15: a keyboard way to
 * move a node), handled by the tree, not the window. */
export const LAYER_MOVE = {
  up: { key: "arrowup", alt: true },
  down: { key: "arrowdown", alt: true },
} as const satisfies Record<string, Chord>;

export const isMac = (): boolean =>
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

/** What a key press reads as: the fields of a `KeyboardEvent` matching uses. */
export interface Press {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

/** Where focus is when the key is pressed. `text`: a text field, where the
 * platform's own keys (undo, copy, arrows) edit the field; `widget`: a
 * button, tab, tree, menu or other control with its own keys, where only
 * modified chords and tool keys reach the designer; `canvas`: the canvas or
 * nothing in particular. */
export type Focus = "text" | "widget" | "canvas";

export function matches(chord: Chord, press: Press, mac: boolean): boolean {
  const key = press.key.toLowerCase();
  if (key !== chord.key) return false;
  const mod = mac ? press.metaKey && !press.ctrlKey : press.ctrlKey && !press.metaKey;
  const none = !press.ctrlKey && !press.metaKey;
  if (chord.mod === true ? !mod : !none) return false;
  if (chord.shift !== "any" && press.shiftKey !== (chord.shift ?? false)) return false;
  return press.altKey === (chord.alt ?? false);
}

/** The shortcut a key press fires where focus is, or `null`. While a
 * gesture is open, unmodified keys go to the tool (numeric entry types
 * `x` and digits: 07 §4) rather than switching tools. */
export function find(press: Press, focus: Focus, mac: boolean, gesture = false): Shortcut | null {
  for (const s of SHORTCUTS) {
    if (s.pending !== undefined) continue;
    if (focus === "text" && s.inFields !== true) continue;
    if (focus === "widget" && s.route === "input") continue;
    if (gesture && s.route === "tool") continue;
    if (s.chords.some((c) => matches(c, press, mac))) return s;
  }
  return null;
}

const NAMES: Record<string, string> = {
  arrowleft: "←",
  arrowright: "→",
  arrowup: "↑",
  arrowdown: "↓",
  escape: "Esc",
  enter: "↵",
  delete: "Del",
  backspace: "⌫",
  "-": "−",
};

/** A chord as the platform writes it: `⌘⇧Z` on macOS, `Ctrl+Shift+Z` elsewhere. */
export function display(chord: Chord, mac: boolean): string {
  const key = NAMES[chord.key] ?? chord.key.toUpperCase();
  const shift = chord.shift === true;
  if (mac)
    return `${chord.mod === true ? "⌘" : ""}${chord.alt === true ? "⌥" : ""}${shift ? "⇧" : ""}${key}`;
  const parts = [
    chord.mod === true ? "Ctrl" : "",
    chord.alt === true ? "Alt" : "",
    shift ? "Shift" : "",
    key,
  ];
  return parts.filter((p) => p !== "").join("+");
}

/** The first chord of shortcut `id`, displayed, for a tooltip. */
export function keyOf(id: string, mac = isMac()): string | null {
  const chord = SHORTCUTS.find((s) => s.id === id)?.chords[0];
  return chord === undefined ? null : display(chord, mac);
}
