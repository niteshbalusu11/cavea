// The shortcut table bound to the window, and the shell's actions behind it.
import type { Key, Modifiers, SnapSettings, ToolId } from "@cavea/designer-core";
import type { Editor } from "./editor.ts";
import { find, isMac, type Focus } from "./shortcuts.ts";

const NAMED: Record<string, Key> = {
  Escape: "escape",
  Enter: "enter",
  ArrowLeft: "arrowLeft",
  ArrowRight: "arrowRight",
  ArrowUp: "arrowUp",
  ArrowDown: "arrowDown",
  Backspace: "backspace",
  Delete: "delete",
};

/** A DOM key as the edit core's `Key`, or `null` for one it does not take. */
export function toKey(key: string): Key | null {
  return NAMED[key] ?? (key.length === 1 ? { char: key.toLowerCase() } : null);
}

export const modifiers = (e: KeyboardEvent | PointerEvent, mac = isMac()): Modifiers => ({
  shift: e.shiftKey,
  alt: e.altKey,
  ctrl: mac ? e.metaKey : e.ctrlKey,
});

const TEXT_INPUTS = new Set(["text", "search", "number", "email", "url", "password", "tel"]);

export function focusOf(target: EventTarget | null): Focus {
  if (!(target instanceof HTMLElement) || target === document.body) return "canvas";
  if (
    target.isContentEditable ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
  ) {
    return "text";
  }
  if (target instanceof HTMLInputElement) return TEXT_INPUTS.has(target.type) ? "text" : "widget";
  return target.closest("[data-canvas]") !== null ? "canvas" : "widget";
}

/** The shell's own dialogs a shortcut opens. */
export interface Shell {
  save: () => void;
  palette: () => void;
  help: () => void;
}

/** The shell's side of each live shortcut of the table, by id. */
export function actions(editor: Editor, shell: Shell): Record<string, () => void> {
  const sel = () => editor.views.selection;
  const snap = (change: (s: SnapSettings) => Partial<SnapSettings>) => {
    const s = editor.views.tool.snap;
    editor.run((x) => x.setSnap({ ...s, ...change(s) }));
  };
  return {
    undo: () => editor.run((s) => s.undo()),
    redo: () => editor.run((s) => s.redo()),
    copy: () => void editor.copy(false),
    cut: () => void editor.copy(true),
    paste: () => void editor.paste(),
    duplicate: () => editor.reveal(editor.run((s) => s.duplicate([50, 50]))?.created[0]),
    selectAll: () => editor.run((s) => s.selectAll()),
    // Ctrl+L and Ctrl+H unlock (show) when every selected node already is (07 §5.1).
    lock: () => editor.run((s) => s.setLocked(sel().locked < sel().count)),
    hide: () => editor.run((s) => s.setHidden(sel().hidden < sel().count)),
    zoomIn: () => editor.zoomBy(1.25),
    zoomOut: () => editor.zoomBy(1 / 1.25),
    zoomFit: () => editor.frame(editor.run((s) => s.fit()) ?? null),
    zoomSelection: () => editor.frame(sel().bounds),
    snap: () =>
      snap((s) => {
        const on = !(s.objects || s.spacing || s.rows || s.angles || s.guides);
        return { objects: on, spacing: on, rows: on, angles: on, guides: on };
      }),
    grid: () => snap((s) => ({ grid: !s.grid })),
    ...shell,
  };
}

/** Installs the window's key handling; returns the uninstall. */
export function bindKeys(editor: Editor, act: Record<string, () => void>): () => void {
  const mac = isMac();
  /** Keys whose key down went to the tool, so that their key up does too. */
  const held = new Map<string, Key>();
  const send = (kind: "keyDown" | "keyUp", e: KeyboardEvent, key: Key) => {
    if (kind === "keyDown") held.set(e.key.toLowerCase(), key);
    else if (!held.delete(e.key.toLowerCase())) return;
    editor.key(kind, key, modifiers(e, mac));
  };
  const down = (e: KeyboardEvent) => {
    if (e.defaultPrevented || e.isComposing) return;
    if (
      e.target instanceof Element &&
      e.target.closest("[role=dialog],[role=alertdialog],[role=menu],[role=listbox]")
    ) {
      return;
    }
    const focus = focusOf(e.target);
    const gesture = editor.views.history.gesture;
    const shortcut = find(e, focus, mac, gesture);
    const key = toKey(e.key);
    if (shortcut === null) {
      // Other printable keys on the canvas go to the tool: Space holds the
      // hand tool, and numeric entry types digits and `x` (07 §4).
      if (
        focus === "canvas" &&
        key !== null &&
        typeof key === "object" &&
        !e.ctrlKey &&
        !e.metaKey
      ) {
        e.preventDefault();
        send("keyDown", e, key);
      }
      return;
    }
    e.preventDefault();
    if (shortcut.route === "input" && key !== null) send("keyDown", e, key);
    else if (shortcut.route === "tool") editor.setTool(shortcut.id.slice("tool.".length) as ToolId);
    else act[shortcut.id]?.();
  };
  const up = (e: KeyboardEvent) => {
    const key = toKey(e.key);
    if (key !== null) send("keyUp", e, key);
  };
  // A key released in another window sends no key up here: release what
  // the tool holds (Space's hand tool) when the window loses focus.
  const blur = () => {
    for (const key of held.values())
      editor.key("keyUp", key, { shift: false, alt: false, ctrl: false });
    held.clear();
  };
  window.addEventListener("keydown", down);
  window.addEventListener("keyup", up);
  window.addEventListener("blur", blur);
  return () => {
    window.removeEventListener("keydown", down);
    window.removeEventListener("keyup", up);
    window.removeEventListener("blur", blur);
  };
}
