// The manager's rules: what an action may take, how a refusal names its
// seats, what the map is told as statuses and the selection change, how a
// whole read merges with fresher reads, and when to read everything.
import { expect, test } from "bun:test";
import { ApiError } from "../../api/client.ts";
import type { EventObject } from "../../api/generated.ts";
import {
  ACTIONS,
  blockers,
  deltaFor,
  eligible,
  mergeRead,
  refusedSeats,
  statusesToRead,
  wholeReadDue,
  WHOLE_EVERY_MS,
  type Status,
} from "./model.ts";

const STATUSES: Status[] = ["free", "held", "booked", "blocked", 7];

test("each action takes seats only from the status 02 §4.3 allows; a held seat from none", () => {
  const takes = Object.fromEntries(ACTIONS.map((a) => [a, STATUSES.filter((s) => eligible(a, s))]));
  expect(takes).toEqual({
    block: ["free"],
    unblock: ["blocked", 7],
    book: ["free"],
    release: ["booked"],
  });
  const seats = [
    { label: "A-1", status: "free" as Status },
    { label: "A-2", status: "held" as Status },
  ];
  expect(blockers("block", seats).map((s) => s.label)).toEqual(["A-2"]);
  expect(blockers("block", seats.slice(0, 1))).toEqual([]);
});

test("a refusal maps back to the seats it names, with the engine's reason", () => {
  const conflict = new ApiError(
    409,
    "object_not_free",
    null,
    {
      objects: [
        { label: "A-1", oid: 3, reason: "not_free" },
        { idx: 4, reason: "not_free" },
        "garbage",
      ],
    },
    "req_x",
    null,
    "",
  );
  expect(refusedSeats(conflict)).toEqual([{ label: "A-1", reason: "not_free" }]);
  // Unknown labels come without a reason: the code stands for it.
  const missing = new ApiError(
    404,
    "not_found",
    null,
    {
      missing: "objects",
      objects: [{ label: "Z-9" }],
    },
    null,
    null,
    "",
  );
  expect(refusedSeats(missing)).toEqual([{ label: "Z-9", reason: "not_found" }]);
  const other = new ApiError(403, "forbidden", "role", {}, null, null, "");
  expect(refusedSeats(other)).toEqual([]);
});

test("the map is told only what changed: taken seats unavailable, selected ones available", () => {
  const nonFree = new Set(["A-1", "A-2"]);
  const selected = new Set<string>();
  const hidden = (l: string) => nonFree.has(l) && !selected.has(l);
  const drawn = new Set<string>();

  expect(deltaFor(["A-1", "A-2", "A-3"], drawn, hidden)).toEqual({
    unavailable: ["A-1", "A-2"],
    available: [],
  });
  // Nothing changed: nothing to tell (no frame drawn for it).
  expect(deltaFor(["A-1", "A-2", "A-3"], drawn, hidden)).toEqual({
    unavailable: [],
    available: [],
  });

  // The operator selects a booked seat: it is drawn available so the
  // picker keeps it; deselected, it goes back.
  selected.add("A-1");
  expect(deltaFor(["A-1"], drawn, hidden)).toEqual({ unavailable: [], available: ["A-1"] });
  // A buyer takes a selected free seat: it stays drawn selectable.
  nonFree.add("A-3");
  selected.add("A-3");
  expect(deltaFor(["A-3"], drawn, hidden)).toEqual({ unavailable: [], available: [] });
  selected.clear();
  expect(deltaFor(["A-1", "A-3"], drawn, hidden)).toEqual({
    unavailable: ["A-1", "A-3"],
    available: [],
  });
  // Released elsewhere: available again.
  nonFree.delete("A-2");
  expect(deltaFor(["A-2"], drawn, hidden)).toEqual({ unavailable: [], available: ["A-2"] });
});

const o = (label: string, status: EventObject["status"]): EventObject => ({
  label,
  oid: 0,
  status,
});

test("a whole read does not undo what was learnt seat by seat after it began", () => {
  const read = new Map([
    ["A-1", o("A-1", "held")],
    ["A-2", o("A-2", "booked")],
    ["A-3", o("A-3", "blocked")],
  ]);
  // Since the read began at 100: A-1 was booked (an action), A-2 released
  // (read by label as free), and A-4 blocked; A-3 was learnt before it.
  const current = new Map([
    ["A-1", o("A-1", "booked")],
    ["A-4", o("A-4", "blocked")],
  ]);
  const learnt = new Map([
    ["A-1", 150],
    ["A-2", 120],
    ["A-3", 50],
    ["A-4", 100],
  ]);
  const merged = mergeRead(read, current, learnt, 100);
  expect(Object.fromEntries([...merged].map(([l, v]) => [l, v.status]))).toEqual({
    "A-1": "booked",
    "A-3": "blocked",
    "A-4": "blocked",
  });
});

test("a whole read pages only statuses some seat has, and waits in proportion to its cost", () => {
  expect(
    statusesToRead({ free: 10, held: 0, booked: 2, blocked: 0, custom: { "5": 1, "6": 0 } }),
  ).toEqual(["booked", "5"]);
  expect(statusesToRead({ free: 10, held: 0, booked: 0, blocked: 0, custom: {} })).toEqual([]);

  expect(wholeReadDue(0, null, false)).toBe(true);
  const quick = { endedAt: 1_000, tookMs: 50 };
  expect(wholeReadDue(2_000, quick, true)).toBe(false); // within the 2 s gap
  expect(wholeReadDue(3_000, quick, true)).toBe(true);
  expect(wholeReadDue(3_000, quick, false)).toBe(false); // nothing changed
  expect(wholeReadDue(1_000 + WHOLE_EVERY_MS, quick, false)).toBe(true);
  // A read that took 3 s waits 30 s even when the counts move.
  const slow = { endedAt: 1_000, tookMs: 3_000 };
  expect(wholeReadDue(20_000, slow, true)).toBe(false);
  expect(wholeReadDue(31_000, slow, true)).toBe(true);
});
