// The event manager's rules as plain functions (09 §4.2, CON-008; D-67):
// which seats an action may take, how a refusal maps back to seats, what
// the map must be told when the truth or the selection changes, how a
// whole read of the statuses merges with what was learnt meanwhile, and
// when to read everything again. The screen and the poller only call these.
import type { ApiError } from "../../api/client.ts";
import type { Counts, EventObject } from "../../api/generated.ts";

export type Status = EventObject["status"];
export type Action = "block" | "unblock" | "book" | "release";
export const ACTIONS: readonly Action[] = ["block", "unblock", "book", "release"];

/** Whether `action` may take a seat in `status` (02 §4.3): block and book
 * a free seat, unblock a blocked or custom one, release a booked one. A
 * held seat is the buyer's until it is booked with the token or expires. */
export function eligible(action: Action, status: Status): boolean {
  switch (action) {
    case "block":
    case "book":
      return status === "free";
    case "unblock":
      return status === "blocked" || typeof status === "number";
    case "release":
      return status === "booked";
  }
}

/** The seats of `seats` that stop `action`: all-or-nothing on the server,
 * so the action is offered only when this is empty. */
export function blockers<S extends { status: Status }>(action: Action, seats: readonly S[]): S[] {
  return seats.filter((s) => !eligible(action, s.status));
}

export interface Refusal {
  label: string;
  /** The engine's reason (`not_free`, `not_booked`, `not_blocked`), or the
   * problem's code when it gives none (`not_found` for an unknown label). */
  reason: string;
}

/** The objects a refused seat command names (08 §6.4: `{label, oid,
 * reason}` per object; `not_found` lists unresolved labels alone). Empty
 * when the refusal is not about particular seats. */
export function refusedSeats(e: ApiError): Refusal[] {
  const list = e.details["objects"];
  if (!Array.isArray(list)) return [];
  const out: Refusal[] = [];
  for (const o of list as unknown[]) {
    if (typeof o !== "object" || o === null) continue;
    const r = o as Record<string, unknown>;
    if (typeof r["label"] !== "string") continue;
    out.push({ label: r["label"], reason: typeof r["reason"] === "string" ? r["reason"] : e.code });
  }
  return out;
}

export interface LabelDelta {
  unavailable: string[];
  available: string[];
}

/**
 * What the map must be told about `labels` so that it draws unavailable
 * exactly the seats `unavailable` says, given those it draws so now
 * (`drawn`, updated in place). The picker refuses a seat it draws
 * unavailable, so a seat the operator selects is drawn available whatever
 * its status: `unavailable` is "not free and not selected".
 */
export function deltaFor(
  labels: Iterable<string>,
  drawn: Set<string>,
  unavailable: (label: string) => boolean,
): LabelDelta {
  const delta: LabelDelta = { unavailable: [], available: [] };
  for (const label of labels) {
    const want = unavailable(label);
    if (want === drawn.has(label)) continue;
    if (want) {
      drawn.add(label);
      delta.unavailable.push(label);
    } else {
      drawn.delete(label);
      delta.available.push(label);
    }
  }
  return delta;
}

/**
 * The seats that are not free after a whole read that began at `started`:
 * the read's answer, except for seats learnt about one by one since it
 * began (a seat read by label, or changed by the operator's own action),
 * which keep what was learnt, being fresher than the read's page.
 */
export function mergeRead(
  read: ReadonlyMap<string, EventObject>,
  current: ReadonlyMap<string, EventObject>,
  learnt: ReadonlyMap<string, number>,
  started: number,
): Map<string, EventObject> {
  const out = new Map<string, EventObject>();
  for (const [label, object] of read) {
    if ((learnt.get(label) ?? -Infinity) < started) out.set(label, object);
  }
  for (const [label, at] of learnt) {
    if (at < started) continue;
    const object = current.get(label);
    if (object !== undefined) out.set(label, object);
  }
  return out;
}

/** The statuses a whole read must page through: those the counts say some
 * seat has (free seats have no row, and are every seat not listed). */
export function statusesToRead(counts: Counts): string[] {
  const out: string[] = [];
  if (counts.held > 0) out.push("held");
  if (counts.booked > 0) out.push("booked");
  if (counts.blocked > 0) out.push("blocked");
  for (const [code, n] of Object.entries(counts.custom)) if (n > 0) out.push(code);
  return out;
}

export const sameCounts = (a: Counts, b: Counts): boolean =>
  a.free === b.free &&
  a.held === b.held &&
  a.booked === b.booked &&
  a.blocked === b.blocked &&
  JSON.stringify(a.custom) === JSON.stringify(b.custom);

/** How often the counts and the watched seats (the selection and the seat
 * inspected) are read. */
export const TICK_MS = 3_000;
/** A whole read at least this often, for changes the counts cannot show
 * (one seat held as another's hold expires). */
export const WHOLE_EVERY_MS = 60_000;
/** A whole read waits at least this long after the last, and at least
 * `WHOLE_GAP_FACTOR` times as long as the last took: a large, busy event is
 * then read less often instead of keeping the server reading. */
export const WHOLE_GAP_MS = 2_000;
export const WHOLE_GAP_FACTOR = 10;

/** Whether a whole read is due at `now`, after the last one (`null`:
 * none yet), when the counts differ from those that read began with. */
export function wholeReadDue(
  now: number,
  last: { endedAt: number; tookMs: number } | null,
  countsChanged: boolean,
): boolean {
  if (last === null) return true;
  const since = now - last.endedAt;
  if (since < Math.max(WHOLE_GAP_MS, WHOLE_GAP_FACTOR * last.tookMs)) return false;
  return countsChanged || since >= WHOLE_EVERY_MS;
}

/** At most this many labels named in a list ("A-1, A-2 and 6 more"). */
export const NAMED = 8;

/** `labels` for a sentence: the first `NAMED`, and how many more. */
export function namedSeats(labels: readonly string[]): { named: string; more: number } {
  return { named: labels.slice(0, NAMED).join(", "), more: Math.max(0, labels.length - NAMED) };
}
