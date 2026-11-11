// User guides kept beside a server draft: what a session saves is read back
// whole into a new session of the same chart, per floor; anything else
// stored is refused rather than half restored.
import { beforeAll, expect, test } from "bun:test";
import type { Designer } from "@cavea/designer-core";
import { parseGuides, throttledWriter } from "./guides.ts";
import { designer } from "./testkit.ts";

let d: Designer;
beforeAll(async () => {
  d = await designer();
});

const chart = JSON.stringify({
  schema: 1,
  name: "g",
  settings: {},
  nextOid: 1,
  categories: [],
  floors: [
    { id: 1, name: "Stalls" },
    { id: 2, name: "Circle" },
  ],
  nodes: [],
});

test("guides stored and restored per floor, through the storage's JSON", () => {
  const a = d.open(chart);
  a.addGuide({ axis: "vertical", at: 120.5 });
  a.addGuide({ axis: "horizontal", at: -40 });
  a.setFloor(2);
  a.addGuide({ axis: "vertical", at: 7 });
  const stored = JSON.parse(JSON.stringify(a.savedGuides())) as unknown;

  const b = d.open(chart);
  const parsed = parseGuides(stored);
  expect(parsed).toBeDefined();
  b.restoreGuides(parsed ?? []);
  expect(b.savedGuides()).toEqual(a.savedGuides());
  expect(b.guides()).toEqual([
    { axis: "vertical", at: 120.5 },
    { axis: "horizontal", at: -40 },
  ]);
  b.setFloor(2);
  expect(b.guides()).toEqual([{ axis: "vertical", at: 7 }]);
  // Undo never touches them.
  expect(b.history().undo).toBeNull();
  a.dispose();
  b.dispose();
});

test("anything else stored is refused whole", () => {
  for (const raw of [
    null,
    {},
    [{ floor: 1 }],
    [{ floor: -1, guides: [] }],
    [{ floor: 1.5, guides: [] }],
    [{ floor: 1, guides: [{ axis: "diagonal", at: 1 }] }],
    [{ floor: 1, guides: [{ axis: "vertical", at: "1" }] }],
    [{ floor: null, guides: Array.from({ length: 257 }, () => ({ axis: "vertical", at: 1 })) }],
  ]) {
    expect(parseGuides(raw)).toBeUndefined();
  }
  // Infinity cannot be stored as JSON; a session refuses a non-finite one.
  expect(parseGuides([{ floor: null, guides: [{ axis: "vertical", at: 1e400 }] }])).toBeUndefined();
});

test("written at most once per delay, the latest, and at once on flush", () => {
  const writes: unknown[] = [];
  let pending: (() => void) | null = null;
  const w = throttledWriter((g) => writes.push(g), 500, {
    set: (run) => {
      pending = run;
      return 1;
    },
    clear: () => (pending = null),
  });
  w.put(() => [{ floor: null, guides: [] }]);
  w.put(() => [{ floor: 1, guides: [] }]);
  expect(writes).toEqual([]);
  (pending as (() => void) | null)?.();
  expect(writes).toEqual([[{ floor: 1, guides: [] }]]);
  w.put(() => [{ floor: 2, guides: [] }]);
  w.flush();
  expect(writes.length).toBe(2);
  w.flush();
  expect(writes.length).toBe(2);
});
