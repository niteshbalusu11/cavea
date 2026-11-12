// The event manager's rules as plain functions (09 §4.2, CON-008; D-68,
// D-82): which seats an action may take, how a refusal maps back to seats,
// what the map must be told when the truth or the selection changes, and
// how the operator's own action shows until the state catches up. The
// screen and the poller only call these.
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

/** A domain status's code (02 §4.1) as the API names it. */
export function statusOf(code: number): Status {
  switch (code) {
    case 0:
      return "free";
    case 1:
      return "held";
    case 2:
      return "booked";
    case 3:
      return "blocked";
    default:
      return code;
  }
}

/** The event's counts as the state shows them, one pass over the codes. */
export function countsOf(codes: Uint8Array): Counts {
  const n: number[] = Array.from({ length: 16 }, () => 0);
  for (const c of codes) n[c] = (n[c] ?? 0) + 1;
  const custom: Record<string, number> = {};
  for (let code = 4; code < 16; code++) if ((n[code] ?? 0) > 0) custom[String(code)] = n[code] ?? 0;
  return { free: n[0] ?? 0, held: n[1] ?? 0, booked: n[2] ?? 0, blocked: n[3] ?? 0, custom };
}

/** What the operator's own action made of a seat, until the state shows
 * it: the reply's `seq`, which the state reaches within a publication. */
export interface Pending {
  status: Status;
  seq: number;
}

/**
 * A seat's status: the operator's own action's result while the state is
 * older than it (so a poll answered before the change cannot undo it on
 * screen), else what the state says. Drops the pending entries the state
 * has reached.
 */
export function seatStatus(
  pending: Map<string, Pending>,
  stateSeq: number | null,
  label: string,
  fromState: () => Status,
): Status {
  const p = pending.get(label);
  if (p !== undefined) {
    if (stateSeq === null || stateSeq < p.seq) return p.status;
    pending.delete(label);
  }
  return fromState();
}

/**
 * A details answer for `labels` (`objects`: those of them not free), sent
 * as read number `at`, into `details` and `readAt` in place: a seat whose
 * details came from a later read keeps them, so an answer that arrives
 * late (sent before a hold was extended) cannot restore what it replaced.
 */
export function takeDetails(
  details: Map<string, EventObject>,
  readAt: Map<string, number>,
  labels: readonly string[],
  objects: readonly EventObject[],
  at: number,
): void {
  const given = new Map(objects.map((o) => [o.label, o]));
  for (const label of labels) {
    if ((readAt.get(label) ?? 0) > at) continue;
    readAt.set(label, at);
    const o = given.get(label);
    if (o === undefined) details.delete(label);
    else details.set(label, o);
  }
}

/** Seats whose details read number `at` failed, in `unread` (seat → the
 * failed read's number): read again until a later read succeeds. */
export function markUnread(unread: Map<string, number>, labels: readonly string[], at: number) {
  for (const label of labels) unread.set(label, Math.max(unread.get(label) ?? 0, at));
}

/** Read number `at` succeeded for `labels`: a seat whose read failed
 * after it was sent is still to be read again. */
export function markRead(unread: Map<string, number>, labels: readonly string[], at: number) {
  for (const label of labels) if ((unread.get(label) ?? 0) < at) unread.delete(label);
}

/** At most this many labels named in a list ("A-1, A-2 and 6 more"). */
export const NAMED = 8;

/** `labels` for a sentence: the first `NAMED`, and how many more. */
export function namedSeats(labels: readonly string[]): { named: string; more: number } {
  return { named: labels.slice(0, NAMED).join(", "), more: Math.max(0, labels.length - NAMED) };
}
