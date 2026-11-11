import { expect, test } from "bun:test";
import { DEFAULT_LAYOUT, parseLayout, SIDE } from "./layout.ts";

test("a stored layout is read field by field, and garbage falls back to the default", () => {
  for (const raw of [null, 42, "x", [], { side: "wide" }, { tab: "numbering" }]) {
    expect(parseLayout(raw)).toEqual(DEFAULT_LAYOUT);
  }
  expect(
    parseLayout({ side: 1e9, split: Number.NaN, panelsOpen: false, tab: "validation", extra: 1 }),
  ).toEqual({
    ...DEFAULT_LAYOUT,
    side: SIDE.max,
    panelsOpen: false,
    tab: "validation",
  });
});
