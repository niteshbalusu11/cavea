// A read-only session refuses every change, whatever calls it, and its
// document is untouched by the pointer and keys (09 §2: read, not write).
import type { Input, Pointer } from "@cavea/designer-core";
import { expect, test } from "bun:test";
import { makeReadOnly, ReadOnlyRefusal } from "./readonly.ts";
import { designer } from "./testkit.ts";

test("a read-only session can be looked at and not changed", async () => {
  const d = await designer();
  const text = await Bun.file(new URL("./templates/theatre.json", import.meta.url)).text();
  const s = d.open(text);
  makeReadOnly(s, "read-only");
  const before = s.export();
  const revision = s.status().revision;
  s.selectAll();
  expect(s.selectionView().count).toBeGreaterThan(0);
  for (const change of [
    () => s.delete(),
    () => s.duplicate([10, 10]),
    () => s.setField({ field: "chartName", value: "Changed" }),
    () => s.undo(),
    () => s.setTool("select"),
    () => s.beginScrub(),
  ]) {
    expect(change).toThrow(ReadOnlyRefusal);
  }
  // Guides are the viewer's own, beside the draft: kept and restored.
  s.restoreGuides([{ floor: s.floors().shown, guides: [{ axis: "vertical", at: 5 }] }]);
  s.addGuide({ axis: "horizontal", at: 7 });
  expect(s.guides().length).toBe(2);
  expect(s.export()).toBe(before);
  // A drag across the chart, and Delete: the hand tool pans, keys go nowhere.
  const fit = s.fit();
  if (fit === null) throw new Error("the theatre has no extent");
  const [x0, y0] = fit.min;
  const width = fit.max[0] - x0;
  const y = (y0 + fit.max[1]) / 2;
  const at = (x: number, time: number): Pointer => ({
    world: [x0 + x, y],
    screen: [x, 300],
    zoom: 0.1,
    button: "primary",
    kind: "mouse",
    mods: { shift: false, alt: false, ctrl: false },
    time,
  });
  const inputs: Input[] = [
    { kind: "pointerDown", data: at(width / 2, 0) },
    { kind: "pointerMove", data: at(width / 2 + 500, 16) },
    { kind: "pointerUp", data: at(width / 2 + 500, 32) },
    { kind: "keyDown", data: ["delete", { shift: false, alt: false, ctrl: false }] },
    { kind: "keyUp", data: ["delete", { shift: false, alt: false, ctrl: false }] },
  ];
  for (const i of inputs) s.input(i);
  expect(s.status().revision).toBe(revision);
  expect(s.export()).toBe(before);
  s.dispose();
});
