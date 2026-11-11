// The designer's keyboard shortcuts (07 §4, §8) as one table: it binds the
// keys, and it feeds the tooltips, the command palette and the `?` help
// (DSN-009). On macOS, Cmd replaces Ctrl. An action that does not exist
// yet is in the table as `pending`, with the task that brings it, and is
// bound to nothing.
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

/** The help's groups, in its order (07 §8, §4). */
export const GROUPS = ["edit", "select", "arrange", "view", "general", "tools"] as const;
export type Group = (typeof GROUPS)[number];

export interface Shortcut {
  id: string;
  label: MessageId;
  chords: Chord[];
  route: Route;
  group: Group;
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
  group: "tools",
});
const later = (
  id: string,
  label: MessageId,
  group: Group,
  chords: Chord[],
  task: string,
): Shortcut => ({ id, label, group, chords, route: "call", pending: task });
const arrows = ["arrowleft", "arrowright", "arrowup", "arrowdown"];

export const SHORTCUTS: readonly Shortcut[] = [
  {
    id: "undo",
    group: "edit",
    label: "shortcut.undo",
    chords: [{ key: "z", mod: true }],
    route: "call",
  },
  {
    id: "redo",
    group: "edit",
    label: "shortcut.redo",
    chords: [{ key: "z", mod: true, shift: true }],
    route: "call",
  },
  {
    id: "copy",
    group: "edit",
    label: "shortcut.copy",
    chords: [{ key: "c", mod: true }],
    route: "call",
  },
  {
    id: "cut",
    group: "edit",
    label: "shortcut.cut",
    chords: [{ key: "x", mod: true }],
    route: "call",
  },
  {
    id: "paste",
    group: "edit",
    label: "shortcut.paste",
    chords: [{ key: "v", mod: true }],
    route: "call",
  },
  {
    id: "duplicate",
    group: "edit",
    label: "shortcut.duplicate",
    chords: [{ key: "d", mod: true }],
    route: "call",
  },
  {
    id: "selectAll",
    group: "select",
    label: "shortcut.selectAll",
    chords: [{ key: "a", mod: true }],
    route: "call",
  },
  {
    id: "lock",
    group: "arrange",
    label: "shortcut.lock",
    chords: [{ key: "l", mod: true }],
    route: "call",
  },
  {
    id: "hide",
    group: "arrange",
    label: "shortcut.hide",
    chords: [{ key: "h", mod: true }],
    route: "call",
  },
  {
    id: "zoomIn",
    group: "view",
    label: "shortcut.zoomIn",
    chords: [
      { key: "=", mod: true, shift: "any" },
      { key: "+", mod: true, shift: "any" },
    ],
    route: "call",
  },
  {
    id: "zoomOut",
    group: "view",
    label: "shortcut.zoomOut",
    chords: [{ key: "-", mod: true }],
    route: "call",
  },
  {
    id: "zoomFit",
    group: "view",
    label: "shortcut.zoomFit",
    chords: [{ key: "0", mod: true }],
    route: "call",
  },
  {
    id: "zoomSelection",
    group: "view",
    label: "shortcut.zoomSelection",
    chords: [{ key: "2", mod: true }],
    route: "call",
  },
  {
    id: "snap",
    group: "view",
    label: "shortcut.snap",
    chords: [{ key: "'", mod: true }],
    route: "call",
  },
  {
    id: "grid",
    group: "view",
    label: "shortcut.grid",
    chords: [{ key: ";", mod: true }],
    route: "call",
  },
  {
    id: "save",
    group: "general",
    label: "shortcut.save",
    chords: [{ key: "s", mod: true }],
    route: "call",
    inFields: true,
  },
  {
    id: "delete",
    group: "edit",
    label: "shortcut.delete",
    chords: [{ key: "delete" }, { key: "backspace" }],
    route: "input",
  },
  {
    id: "deselect",
    group: "select",
    label: "shortcut.deselect",
    chords: [{ key: "escape" }],
    route: "input",
  },
  {
    id: "stepIn",
    group: "select",
    label: "shortcut.stepIn",
    chords: [{ key: "enter" }],
    route: "input",
  },
  {
    id: "nudge",
    group: "select",
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
    "arrange",
    [{ key: "g", mod: true }],
    "07 §5.1: decoration groups need schema 2",
  ),
  later(
    "ungroup",
    "shortcut.ungroup",
    "arrange",
    [{ key: "g", mod: true, shift: true }],
    "07 §5.1: decoration groups need schema 2",
  ),
  {
    id: "palette",
    label: "shortcut.palette",
    group: "general",
    chords: [{ key: "k", mod: true }],
    route: "call",
  },
  {
    id: "help",
    label: "shortcut.help",
    group: "general",
    chords: [{ key: "?", shift: "any" }],
    route: "call",
  },
  {
    id: "rulers",
    label: "shortcut.rulers",
    group: "view",
    chords: [{ key: "r", mod: true }],
    route: "call",
  },
  {
    id: "image",
    label: "shortcut.image",
    group: "tools",
    chords: [{ key: "i" }],
    route: "call",
  },
  later("tool.table", "shortcut.table", "tools", [{ key: "t" }], "DSN-050"),
  later("tool.booth", "shortcut.booth", "tools", [{ key: "b" }], "DSN-051"),
  later("tool.area", "shortcut.area", "tools", [{ key: "g" }], "DSN-052"),
  later("tool.measure", "shortcut.measure", "tools", [{ key: "m" }], "DSN-054"),
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

/** One line of the shortcut help. */
export interface HelpRow {
  /** The table's row, or a layers-tree key's id. */
  id: string;
  label: MessageId;
  keys: string[];
  /** The task that brings it, for a row not bound yet. */
  pending?: string;
}

/** The layers tree's own keys (07 §8), for the help. */
const TREE_KEYS: { id: string; label: MessageId; keys: (mac: boolean) => string[] }[] = [
  { id: "tree.move", label: "help.tree.move", keys: () => ["↑", "↓", "Home", "End"] },
  { id: "tree.page", label: "help.tree.page", keys: () => ["PgUp", "PgDn"] },
  { id: "tree.expand", label: "help.tree.expand", keys: () => ["→", "←"] },
  { id: "tree.select", label: "help.tree.select", keys: () => ["↵", "Space"] },
  {
    id: "tree.extend",
    label: "help.tree.extend",
    keys: (mac) => [mac ? "⇧↵" : "Shift+↵", mac ? "⌘↵" : "Ctrl+↵"],
  },
  {
    id: "tree.reorder",
    label: "help.tree.reorder",
    keys: (mac) => [display(LAYER_MOVE.up, mac), display(LAYER_MOVE.down, mac)],
  },
];

/** The shortcut help (`?`, DSN-009), generated from the table: every row,
 * grouped as 07 §8 and §4 group them, then the layers tree's keys. */
export function helpSections(mac: boolean): { group: Group | "tree"; rows: HelpRow[] }[] {
  const sections: { group: Group | "tree"; rows: HelpRow[] }[] = GROUPS.map((group) => ({
    group,
    rows: SHORTCUTS.filter((s) => s.group === group).map((s) => ({
      id: s.id,
      label: s.label,
      keys: s.chords.map((c) => display(c, mac)),
      ...(s.pending === undefined ? {} : { pending: s.pending }),
    })),
  }));
  sections.push({
    group: "tree",
    rows: TREE_KEYS.map((t) => ({ id: t.id, label: t.label, keys: t.keys(mac) })),
  });
  return sections;
}
