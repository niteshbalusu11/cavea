// The events list's filters: the URL restores what was chosen, and the
// server is asked exactly for it.
import { expect, test } from "bun:test";
import {
  clip,
  DEFAULTS,
  eventsQuery,
  filtered,
  fromParams,
  MAX_Q,
  toParams,
} from "./eventFilters.ts";

test("a URL restores the same view; defaults leave the URL plain; junk is the default", () => {
  const chosen = {
    q: "Hamlet %_",
    chart: "royal",
    shown: "all" as const,
    order: "newest" as const,
  };
  expect(fromParams(toParams(chosen))).toEqual(chosen);
  expect(toParams(DEFAULTS)).toEqual({
    q: undefined,
    chart: undefined,
    shown: undefined,
    order: undefined,
  });
  expect(fromParams({ shown: "deleted", order: ["sideways"], q: ["a", "b"] })).toEqual({
    ...DEFAULTS,
    q: "a",
  });
  expect(filtered(DEFAULTS)).toBe(false);
  expect(filtered({ ...DEFAULTS, q: "  " })).toBe(false);
  expect(filtered({ ...DEFAULTS, order: "newest" })).toBe(false);
  expect(filtered({ ...DEFAULTS, shown: "all" })).toBe(true);
});

test("the server is asked for exactly the view: text as typed, at most 128 characters", () => {
  expect(eventsQuery(DEFAULTS, 25, undefined)).toEqual({ limit: 25, archived: false });
  expect(
    eventsQuery(
      { q: "  50% off_ ", chart: "royal", shown: "archived", order: "newest" },
      25,
      "night-2",
    ),
  ).toEqual({
    limit: 25,
    after: "night-2",
    q: "50% off_",
    chart: "royal",
    archived: true,
    order: "newest",
  });
  expect(eventsQuery({ ...DEFAULTS, shown: "all" }, 25, undefined)).toEqual({ limit: 25 });
  // Characters, not UTF-16 units: an emoji is one of the 128.
  const long = "🎭".repeat(MAX_Q + 5);
  expect([...clip(long)]).toHaveLength(MAX_Q);
  expect([...(eventsQuery({ ...DEFAULTS, q: long }, 25, undefined).q ?? "")]).toHaveLength(MAX_Q);
});
