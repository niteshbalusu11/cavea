// Every action the designer has, for the command palette (07 §8, §15:
// every canvas action has an equivalent in the inspector, the layers panel
// or the palette). The shortcut table's live rows first, by their own
// actions, then what has no key: the other tools, arrange, sections,
// panels, files and the theme. An action that cannot run now says why.
import type { AlignTo, Axis, Order, ToolId } from "@cavea/designer-core";
import { msg, type MessageId } from "../i18n/messages.ts";
import { setThemeChoice } from "../theme.ts";
import type { Editor } from "./editor.ts";
import type { PanelTab } from "./layout.ts";
import { display, isMac, SHORTCUTS, type Group } from "./shortcuts.ts";

export type CommandGroup = Group | "insert" | "panels" | "file" | "theme";

export interface Command {
  id: string;
  label: string;
  group: CommandGroup;
  /** The shortcut, written the platform's way. */
  keys: string | null;
  /** Why it cannot run now, or `null` when it can. */
  reason: () => string | null;
  run: () => void;
}

/** What the shell does for the palette beyond the session. */
export interface Hooks {
  help: () => void;
  insert: () => void;
  numbering: () => void;
  tab: (tab: PanelTab) => void;
  inspector: () => void;
  newChart: () => void;
  open: () => void;
}

const TOOLS_WITHOUT_KEYS: ToolId[] = ["ellipse", "polygon", "line"];
const ALIGN: AlignTo[] = ["left", "centre", "right", "top", "middle", "bottom"];
const AXES: Axis[] = ["horizontal", "vertical"];
const ORDERS: Order[] = ["forward", "backward", "front", "back"];

/** The ids of the selected nodes, or `null` when they are not all listed
 * (`SelectionView.ids` stops at 1,000). */
export function selectedIds(editor: Editor): number[] | null {
  const sel = editor.views.selection;
  return sel.ids.length === sel.count ? [...sel.ids] : null;
}

/** The one selected section's id, or `null`. */
export function selectedSection(editor: Editor): number | null {
  const sel = editor.views.selection;
  const [only] = sel.kinds;
  return sel.level.kind === "top" && sel.count === 1 && only?.kind === "section"
    ? (sel.ids[0] ?? null)
    : null;
}

export function commands(editor: Editor, act: Record<string, () => void>, hooks: Hooks): Command[] {
  const mac = isMac();
  const sel = () => editor.views.selection;
  const nodes = () => sel().level.kind === "top" || sel().level.kind === "section";
  /** A reason unless at least `n` objects are selected. */
  const needs = (n: number) => (): string | null => {
    if (!nodes() || sel().count < n) {
      return n === 1 ? msg("reason.selectObject") : msg("reason.selectObjects", { n });
    }
    return null;
  };
  const anything = () => (sel().count === 0 ? msg("reason.nothingSelected") : null);
  const history = (which: "undo" | "redo") => () => {
    const h = editor.views.history;
    if (h.gesture) return msg("commandError.gestureOpen");
    return h[which] === null
      ? msg(which === "undo" ? "topbar.nothingToUndo" : "topbar.nothingToRedo")
      : null;
  };
  const section = () => (selectedSection(editor) === null ? msg("reason.selectSection") : null);

  /** Why each shortcut action cannot run now; the rest always can. */
  const REASONS: Record<string, () => string | null> = {
    undo: history("undo"),
    redo: history("redo"),
    copy: anything,
    cut: anything,
    duplicate: needs(1),
    lock: needs(1),
    hide: needs(1),
    zoomSelection: () => (sel().bounds === null ? msg("reason.nothingSelected") : null),
    delete: anything,
    deselect: () =>
      sel().count === 0 && sel().level.kind === "top" ? msg("reason.nothingSelected") : null,
    stepIn: () => (sel().count === 1 ? null : msg("reason.selectOne")),
  };
  /** The shortcut table's input keys, run as the session calls they mean
   * from outside the canvas. Nudging is a key by nature: help lists it. */
  const INPUT: Record<string, () => void> = {
    delete: () => editor.run((s) => s.delete()),
    deselect: () => editor.run((s) => s.stepOut()),
    stepIn: () => editor.run((s) => s.stepIn()),
  };

  const out: Command[] = [];
  const add = (
    id: string,
    label: MessageId | string,
    group: CommandGroup,
    run: () => void,
    reason: () => string | null = () => null,
    keys: string | null = null,
  ) => out.push({ id, label, group, run, reason, keys });

  add("insertRowBlock", msg("command.insertRowBlock"), "insert", hooks.insert);
  add("numbering", msg("command.numbering"), "edit", hooks.numbering, () =>
    numberable(editor) ? null : msg("reason.selectBlocks"),
  );
  for (const s of SHORTCUTS) {
    if (s.pending !== undefined || s.id === "palette") continue;
    const run =
      s.route === "tool"
        ? () => editor.setTool(s.id.slice("tool.".length) as ToolId)
        : s.route === "input"
          ? INPUT[s.id]
          : act[s.id];
    if (run === undefined) continue;
    const chord = s.chords[0];
    add(s.id, msg(s.label), s.group, run, REASONS[s.id], chord ? display(chord, mac) : null);
  }
  for (const tool of TOOLS_WITHOUT_KEYS) {
    add(`tool.${tool}`, msg(`tool.${tool}`), "tools", () => editor.setTool(tool));
  }
  for (const to of ALIGN) {
    add(
      `align.${to}`,
      msg(`command.align.${to}`),
      "arrange",
      () => editor.run((s) => s.align(to)),
      needs(2),
    );
  }
  for (const axis of AXES) {
    add(
      `distribute.${axis}`,
      msg(`command.distribute.${axis}`),
      "arrange",
      () => editor.run((s) => s.distribute(axis)),
      needs(3),
    );
    add(
      `flip.${axis}`,
      msg(`command.flip.${axis}`),
      "arrange",
      () => editor.run((s) => s.flip(axis)),
      needs(1),
    );
  }
  for (const order of ORDERS) {
    add(
      `order.${order}`,
      msg(`layers.${order}`),
      "arrange",
      () => editor.run((s) => s.order(order)),
      needs(1),
    );
  }
  add(
    "group",
    msg("layers.group"),
    "arrange",
    () => groupSelection(editor),
    () => groupReason(editor),
  );
  add(
    "dissolve",
    msg("layers.dissolve"),
    "arrange",
    () => {
      const id = selectedSection(editor);
      if (id !== null) editor.run((s) => s.dissolve(id));
    },
    section,
  );
  add(
    "convertOutline",
    msg("command.convertOutline"),
    "arrange",
    () => {
      const id = selectedSection(editor);
      if (id !== null) editor.run((s) => s.convertOutline(id));
    },
    () => section() ?? (outlineAuto(editor) ? null : msg("reason.outlineExplicit")),
  );
  for (const tab of ["layers", "categories", "validation"] as const) {
    add(`panel.${tab}`, msg(`command.panel.${tab}`), "panels", () => hooks.tab(tab));
  }
  add("panel.inspector", msg("command.panel.inspector"), "panels", hooks.inspector);
  add("newChart", msg("menu.new"), "file", hooks.newChart);
  add("open", msg("menu.open"), "file", hooks.open);
  for (const theme of ["system", "light", "dark"] as const) {
    add(`theme.${theme}`, msg(`command.theme.${theme}`), "theme", () => setThemeChoice(theme));
  }
  return out;
}

/** Whether the numbering panel has something to number: row blocks or rows
 * selected, a section (its blocks), or rows or seats of one. */
export function numberable(editor: Editor): boolean {
  const sel = editor.views.selection;
  if (sel.level.kind === "rows" || sel.level.kind === "seats") return true;
  return sel.kinds.some((k) => k.kind === "rowBlock" || k.kind === "row" || k.kind === "section");
}

function outlineAuto(editor: Editor): boolean {
  const sec = editor.views.inspector.section;
  return sec !== null && sec.autoOutline !== "mixed" && sec.autoOutline.same;
}

export function groupReason(editor: Editor): string | null {
  const sel = editor.views.selection;
  if (sel.level.kind !== "top" || sel.count === 0) return msg("reason.selectTopLevel");
  if (sel.kinds.some((k) => k.kind === "section")) return msg("commandError.placement");
  return selectedIds(editor) === null ? msg("reason.tooMany", { n: sel.ids.length }) : null;
}

/** Groups the selected top-level nodes into a section and reveals it. */
export function groupSelection(editor: Editor): void {
  const ids = groupReason(editor) === null ? selectedIds(editor) : null;
  if (ids === null) return;
  editor.reveal(editor.run((s) => s.group(ids, null)));
}
