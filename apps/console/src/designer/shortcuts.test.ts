import { describe, expect, test } from "bun:test";
import { display, find, SHORTCUTS, type Press } from "./shortcuts.ts";

const press = (key: string, mods: Partial<Omit<Press, "key">> = {}): Press => ({
  key,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});
const id = (
  p: Press,
  focus: "text" | "widget" | "canvas" = "canvas",
  mac = false,
  gesture = false,
) => find(p, focus, mac, gesture)?.id ?? null;

describe("the shortcut table", () => {
  test("Ctrl on Windows and Linux is Cmd on macOS, and neither stands in for the other", () => {
    expect(id(press("z", { ctrlKey: true }))).toBe("undo");
    expect(id(press("z", { metaKey: true }), "canvas", true)).toBe("undo");
    expect(id(press("z", { metaKey: true }))).toBeNull();
    expect(id(press("z", { ctrlKey: true }), "canvas", true)).toBeNull();
    expect(id(press("Z", { ctrlKey: true, shiftKey: true }))).toBe("redo");
    expect(id(press("+", { ctrlKey: true, shiftKey: true }))).toBe("zoomIn");
    expect(id(press("=", { ctrlKey: true }))).toBe("zoomIn");
  });

  test("Shift picks a different tool, and a held gesture keeps letters for the tool", () => {
    expect(id(press("r"))).toBe("tool.rowBlock");
    expect(id(press("R", { shiftKey: true }))).toBe("tool.row");
    expect(id(press("x"))).toBe("tool.text");
    expect(id(press("x"), "canvas", false, true)).toBeNull();
    expect(id(press("Escape"), "canvas", false, true)).toBe("deselect");
  });

  test("a text field keeps its own keys; a widget keeps unmodified ones", () => {
    expect(id(press("z", { ctrlKey: true }), "text")).toBeNull();
    expect(id(press("v"), "text")).toBeNull();
    expect(id(press("Backspace"), "text")).toBeNull();
    expect(id(press("s", { ctrlKey: true }), "text")).toBe("save");
    expect(id(press("Enter"), "widget")).toBeNull();
    expect(id(press("ArrowUp"), "widget")).toBeNull();
    expect(id(press("z", { ctrlKey: true }), "widget")).toBe("undo");
    expect(id(press("ArrowUp", { shiftKey: true }))).toBe("nudge");
  });

  test("pending actions are listed and bound to nothing", () => {
    const pending = SHORTCUTS.filter((s) => s.pending !== undefined).map((s) => s.id);
    expect(pending).toContain("palette");
    expect(id(press("k", { ctrlKey: true }))).toBeNull();
    expect(id(press("g", { ctrlKey: true }))).toBeNull();
  });

  test("no two live shortcuts share a chord", () => {
    const seen = new Map<string, string>();
    for (const s of SHORTCUTS.filter((x) => x.pending === undefined)) {
      for (const c of s.chords) {
        const k = JSON.stringify([c.key, c.mod ?? false, c.shift ?? false, c.alt ?? false]);
        expect(seen.get(k) ?? s.id).toBe(s.id);
        seen.set(k, s.id);
      }
    }
  });

  test("chords are written the platform's way", () => {
    expect(display({ key: "z", mod: true, shift: true }, true)).toBe("⌘⇧Z");
    expect(display({ key: "z", mod: true, shift: true }, false)).toBe("Ctrl+Shift+Z");
  });
});
