// The first-run tour moves on when the edit a step teaches lands, and only
// then (07 §16).
import { describe, expect, test } from "bun:test";

import { next, STEPS } from "./tour.ts";

const block = { kinds: [{ kind: "rowBlock" as const, count: 1 }] };
const shape = { kinds: [{ kind: "shape" as const, count: 1 }] };
const done = (undo: string, gesture = false) => ({ undo, gesture });

describe("the tour", () => {
  test("each step ends on its own edit, after it began", () => {
    expect(next(0, 1, 2, done("createNode"), block)).toBe(1);
    expect(next(1, 2, 3, done("curve"), block)).toBe(2);
    expect(next(2, 3, 4, done("seatLabels"), block)).toBe(3);
    expect(next(3, 4, 5, done("category"), block)).toBe(4);
    // The last one is ended by the user.
    expect(next(STEPS.length - 1, 5, 6, done("createNode"), block)).toBe(STEPS.length - 1);
  });

  test("not on an edit from before the step, another edit, a shape or an open gesture", () => {
    expect(next(0, 7, 7, done("createNode"), block)).toBe(0);
    expect(next(1, 2, 3, done("transformNodes"), block)).toBe(1);
    expect(next(0, 1, 2, done("createNode"), shape)).toBe(0);
    expect(next(1, 2, 3, done("curve", true), block)).toBe(1);
  });
});
