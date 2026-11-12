// The events list's filters: the URL restores what was chosen, and the
// server is asked exactly for it.
import { expect, test } from "bun:test";
import {
  clip,
  day,
  dayStart,
  DEFAULTS,
  eventsQuery,
  filtered,
  fromParams,
  instant,
  MAX_Q,
  rangeProblem,
  toParams,
} from "./eventFilters.ts";

test("a URL restores the same view; defaults leave the URL plain; junk is the default", () => {
  const chosen = {
    q: "Hamlet %_",
    chart: "royal",
    shown: "all" as const,
    order: "newest" as const,
    from: "2026-03-03",
    to: "2026-03-05",
  };
  expect(fromParams(toParams(chosen))).toEqual(chosen);
  expect(toParams(DEFAULTS)).toEqual({
    q: undefined,
    chart: undefined,
    shown: undefined,
    order: undefined,
    from: undefined,
    to: undefined,
  });
  expect(fromParams({ shown: "deleted", order: ["sideways"], q: ["a", "b"] })).toEqual({
    ...DEFAULTS,
    q: "a",
  });
  expect(filtered(DEFAULTS)).toBe(false);
  expect(filtered({ ...DEFAULTS, q: "  " })).toBe(false);
  expect(filtered({ ...DEFAULTS, order: "newest" })).toBe(false);
  expect(filtered({ ...DEFAULTS, shown: "all" })).toBe(true);
  expect(filtered({ ...DEFAULTS, to: "2026-03-05" })).toBe(true);
});

test("the server is asked for exactly the view: text as typed, at most 128 characters", () => {
  expect(eventsQuery(DEFAULTS, 25, undefined)).toEqual({ limit: 25, archived: false });
  expect(
    eventsQuery(
      { ...DEFAULTS, q: "  50% off_ ", chart: "royal", shown: "archived", order: "newest" },
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

test("a range in the URL is real days, in order; anything else is not applied", () => {
  expect(fromParams({ from: "2026-02-29", to: "2028-02-29" })).toMatchObject({
    from: "",
    to: "2028-02-29",
  });
  expect(fromParams({ from: "3 March", to: "2026-13-01" })).toMatchObject({ from: "", to: "" });
  expect(fromParams({ from: "0002-03-03" }).from).toBe("");
  // Inverted: the end is dropped, and the form says why before sending.
  expect(fromParams({ from: "2026-03-05", to: "2026-03-03" })).toMatchObject({
    from: "2026-03-05",
    to: "",
  });
  expect(rangeProblem("2026-03-05", "2026-03-03")).not.toBeNull();
  expect(rangeProblem("2026-03-05", "2026-03-05")).toBeNull();
  expect(rangeProblem("", "2026-03-03")).toBeNull();
  expect(day("2024-02-29")).toBe("2024-02-29");
  expect(day("2025-02-29")).toBe("");
  // Days the server's instants do not reach are never sent.
  expect(day("1969-12-31")).toBe("");
  expect(day("1970-06-01")).toBe("");
  expect(day("9999-12-31")).toBe("");
  expect(day("9998-12-31")).toBe("9998-12-31");
});

/** The range sent for `from` to `to` (both included) in `zone`. */
const sent = (from: string, to: string, zone: string) => {
  const q = eventsQuery({ ...DEFAULTS, from, to }, 25, undefined, zone);
  return [q.created_after, q.created_before];
};

test("days are the person's own: their first instants there, the last day included", () => {
  expect(sent("2026-03-03", "2026-03-05", "UTC")).toEqual([
    "2026-03-03T00:00:00Z",
    "2026-03-06T00:00:00Z",
  ]);
  // Half-hour offsets, and a zone ahead of UTC, whose days start the day before in UTC.
  expect(sent("2026-03-03", "2026-03-05", "Asia/Kolkata")).toEqual([
    "2026-03-02T18:30:00Z",
    "2026-03-05T18:30:00Z",
  ]);
  // Behind UTC; each end alone.
  const ny = eventsQuery({ ...DEFAULTS, from: "2026-01-10" }, 25, undefined, "America/New_York");
  expect([ny.created_after, ny.created_before]).toEqual(["2026-01-10T05:00:00Z", undefined]);
});

test("a day with a change of clocks is the day the person lived: 23 or 25 hours", () => {
  // New York springs forward on 8 March 2026 (EST to EDT) and falls back on 1 November.
  expect(sent("2026-03-08", "2026-03-08", "America/New_York")).toEqual([
    "2026-03-08T05:00:00Z",
    "2026-03-09T04:00:00Z",
  ]);
  expect(sent("2026-11-01", "2026-11-01", "America/New_York")).toEqual([
    "2026-11-01T04:00:00Z",
    "2026-11-02T05:00:00Z",
  ]);
  // Auckland leaves summer time on 5 April 2026 (UTC+13 to +12).
  expect(sent("2026-04-05", "2026-04-05", "Pacific/Auckland")).toEqual([
    "2026-04-04T11:00:00Z",
    "2026-04-05T12:00:00Z",
  ]);
  // Santiago skips midnight: 6 September 2026 begins at 01:00 (UTC-4 to -3).
  expect(instant(dayStart("2026-09-06", "America/Santiago"))).toBe("2026-09-06T04:00:00Z");
  expect(sent("2026-09-05", "2026-09-06", "America/Santiago")).toEqual([
    "2026-09-05T04:00:00Z",
    "2026-09-07T03:00:00Z",
  ]);
});
