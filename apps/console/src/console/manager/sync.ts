// How the manager follows the staff state route (08 §8.3, §8.6; D-72,
// D-80), as a plain function of what it has and what a poll answered: the
// poller only carries out the step. A SNAPSHOT replaces everything; a DELTA
// from the `seq` held moves to its `to`; `204` changes nothing. A DELTA
// from any other `seq` is a gap, and a frame that cannot be decoded or does
// not fit the chart is bad: either way nothing of it is applied, the state
// held stays, and the next poll asks for a snapshot (`after_seq=0`).
import { decodeFrame, FrameError } from "./frames.ts";

/** How often the route is polled (08 §8.4 polls every 2 s). */
export const POLL_MS = 2_000;

export type Answer = { status: 204 } | { status: 200; bytes: Uint8Array };

export type Step =
  | { kind: "same" }
  | { kind: "snapshot"; seq: number; codes: Uint8Array }
  | { kind: "delta"; to: number; idx: Uint32Array; codes: Uint8Array }
  /** A delta that does not start at the `seq` held. */
  | { kind: "gap" }
  /** Bytes that break a rule of the wire format or do not fit the chart. */
  | { kind: "bad"; rule: string }
  /** A frame type the route does not send: skipped (08 §8.6). */
  | { kind: "skip" };

/**
 * What to do with a poll's answer, holding `seq` (`null` before the first
 * snapshot) for a chart of `objects` objects.
 */
export function step(seq: number | null, objects: number, answer: Answer): Step {
  if (answer.status === 204) return { kind: "same" };
  let frame;
  try {
    frame = decodeFrame(answer.bytes);
  } catch (e) {
    return { kind: "bad", rule: e instanceof FrameError ? e.rule : "decode" };
  }
  switch (frame.type) {
    case "snapshot":
      // Another object count is another chart version than the map's.
      if (frame.codes.length !== objects) return { kind: "bad", rule: "object count" };
      return { kind: "snapshot", seq: frame.seq, codes: frame.codes };
    case "delta": {
      if (seq === null || frame.from !== seq) return { kind: "gap" };
      // An `idx` at or past the chart's count, of a change or an area,
      // refuses the frame whole (08 §8.6).
      if (frame.end > objects) return { kind: "bad", rule: "idx" };
      return { kind: "delta", to: frame.to, idx: frame.idx, codes: frame.codes };
    }
    case "other":
      return { kind: "skip" };
  }
}

/** The `after_seq` of the next poll: 0 (a snapshot) until one is held or
 * after a gap or a bad frame, else the `seq` held. */
export function nextAfter(seq: number | null, last: Step["kind"] | null): number {
  if (seq === null || last === "gap" || last === "bad") return 0;
  return seq;
}
