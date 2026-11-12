// The manager's rules: what an action may take, how a refusal names its
// seats, what the map is told as statuses and the selection change, and how
// the operator's own action shows until the state catches up.
import { expect, test } from "bun:test";
import { ApiError } from "../../api/client.ts";
import type { EventObject } from "../../api/generated.ts";
import {
  ACTIONS,
  blockers,
  countsOf,
  deltaFor,
  eligible,
  refusedSeats,
  seatStatus,
  statusOf,
  takeDetails,
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

const fromState = () => "free" as Status;

test("the operator's own action shows until the state reaches its seq, then the state rules", () => {
  const pending = new Map([["A-1", { status: "booked" as Status, seq: 10 }]]);
  // A poll answered before the booking (state at 9) does not undo it.
  expect(seatStatus(pending, 9, "A-1", fromState)).toBe("booked");
  expect(seatStatus(pending, null, "A-1", fromState)).toBe("booked");
  // From seq 10 on the state includes it: what it says stands (a buyer's
  // later change included), and the entry is gone.
  expect(seatStatus(pending, 10, "A-1", fromState)).toBe("free");
  expect(pending.size).toBe(0);
  expect(seatStatus(pending, 3, "A-2", () => "held")).toBe("held");
});

test("counts come from the codes, custom statuses by code", () => {
  expect(countsOf(Uint8Array.from([0, 0, 1, 2, 2, 3, 7, 7, 15]))).toEqual({
    free: 2,
    held: 1,
    booked: 2,
    blocked: 1,
    custom: { "7": 2, "15": 1 },
  });
  expect([0, 1, 2, 3, 9].map(statusOf)).toEqual(["free", "held", "booked", "blocked", 9]);
});

/** A1 booked under `ref`. */
const booked = (ref: string): EventObject => ({
  label: "A-1",
  oid: 1,
  status: "booked",
  order_ref: ref,
});

test("a late details answer does not undo a newer one; a seat gone free loses its details", () => {
  const details = new Map<string, EventObject>();
  const readAt = new Map<string, number>();
  // Read 2 (sent later) answers first with the new order; read 1's answer,
  // sent before the seat was booked again, arrives after it.
  takeDetails(details, readAt, ["A-1"], [booked("new-order")], 2);
  takeDetails(details, readAt, ["A-1"], [booked("old-order")], 1);
  expect(details.get("A-1")?.order_ref).toBe("new-order");
  // A newer answer without the seat: it is free, its details go.
  takeDetails(details, readAt, ["A-1"], [], 3);
  expect(details.has("A-1")).toBe(false);
});
