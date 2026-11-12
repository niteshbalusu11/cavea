// The staff state's frames and the chart's labels, against what the server
// itself encoded and resolved (`fixtures/make.ts` says how they were made),
// then against hostile bytes; and the sequence rules the poller follows.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { decodeFrame, FrameError, type DeltaFrame, type SnapshotFrame } from "./frames.ts";
import { ChartError, chartLabels } from "./labels.ts";
import { nextAfter, step } from "./sync.ts";

interface Expected {
  objects: number;
  snapshot: { seq: number; encoding: number; statuses: Record<string, number> };
  delta: { from: number; to: number; changes: Record<string, number> };
}

const fixture = (name: string) => {
  const at = (ext: string) => new URL(`fixtures/${name}.${ext}`, import.meta.url);
  return {
    chart: new Uint8Array(readFileSync(at("ccf"))),
    snapshot: new Uint8Array(readFileSync(at("snapshot"))),
    delta: new Uint8Array(readFileSync(at("delta"))),
    expected: JSON.parse(readFileSync(at("json"), "utf8")) as Expected,
  };
};
const NAMES = ["theatre", "full"];

test("every seat's label and status match what the server encoded, snapshot then delta", () => {
  for (const name of NAMES) {
    const f = fixture(name);
    const labels = chartLabels(f.chart);
    expect(labels).toHaveLength(f.expected.objects);
    expect(new Set(labels).size).toBe(labels.length);
    const snap = decodeFrame(f.snapshot) as SnapshotFrame;
    expect([snap.type, snap.seq, f.snapshot[0]]).toEqual(["snapshot", f.expected.snapshot.seq, 2]);
    const byLabel = (codes: Uint8Array) =>
      Object.fromEntries(labels.flatMap((l, i) => (codes[i] === 0 ? [] : [[l, codes[i]]])));
    expect(byLabel(snap.codes)).toEqual(f.expected.snapshot.statuses);

    const delta = decodeFrame(f.delta) as DeltaFrame;
    expect([delta.type, delta.from, delta.to]).toEqual([
      "delta",
      f.expected.delta.from,
      f.expected.delta.to,
    ]);
    const changes = Object.fromEntries(
      [...delta.idx].map((idx, k) => [labels[idx], delta.codes[k]]),
    );
    expect(changes).toEqual(f.expected.delta.changes);
  }
  // Both snapshot encodings a manager frame may use are covered.
  expect(NAMES.map((n) => fixture(n).expected.snapshot.encoding).toSorted()).toEqual([0, 1]);
});

/** A small deterministic generator, so a failure is reproducible. */
function random(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
  };
}

function mutate(bytes: Uint8Array, r: () => number): Uint8Array {
  const out = [...bytes];
  const edits = 1 + Math.floor(r() * 4);
  for (let e = 0; e < edits; e++) {
    const at = Math.floor(r() * (out.length + 1));
    const kind = Math.floor(r() * 4);
    if (kind === 0) out[at] = Math.floor(r() * 256);
    else if (kind === 1) out.splice(at, 1 + Math.floor(r() * 8));
    else if (kind === 2) out.splice(at, 0, Math.floor(r() * 256));
    else out.length = Math.min(out.length, at);
  }
  return Uint8Array.from(out);
}

test("hostile frames are refused as malformed or decode within the rules, never otherwise", () => {
  const r = random(7);
  const seeds = NAMES.flatMap((n) => [fixture(n).snapshot, fixture(n).delta]);
  let refused = 0;
  for (let i = 0; i < 20_000; i++) {
    const bytes = mutate(seeds[i % seeds.length] ?? new Uint8Array(), r);
    let frame;
    try {
      frame = decodeFrame(bytes);
    } catch (e) {
      expect(e).toBeInstanceOf(FrameError);
      refused += 1;
      continue;
    }
    if (frame.type === "snapshot") {
      expect(frame.codes.every((c) => c <= 15)).toBe(true);
    } else if (frame.type === "delta") {
      expect(frame.to).toBeGreaterThan(frame.from);
      expect(frame.codes.every((c) => c <= 15)).toBe(true);
      expect(frame.idx.every((v, k) => k === 0 || v > (frame.idx[k - 1] ?? 0))).toBe(true);
    }
  }
  expect(refused).toBeGreaterThan(10_000);
});

/** Which rule `bytes` break (`ok`: none). */
const rule = (bytes: number[]) => {
  try {
    decodeFrame(Uint8Array.from(bytes));
    return "ok";
  } catch (e) {
    return e instanceof FrameError ? e.rule : "other";
  }
};

test("the rules a manager decoder must enforce, each on its own", () => {
  // SNAPSHOT seq 1, 3 objects, encoding 0, codes, no areas.
  expect(rule([2, 1, 3, 0, 3, 0, 2, 15, 0])).toBe("ok");
  expect(rule([2, 1, 3, 0, 3, 0, 2, 16, 0])).toBe("code");
  // The public one-bit encoding is never a manager frame's.
  expect(rule([2, 1, 3, 2, 1, 0b011, 0])).toBe("encoding");
  // Runs that do not add up to the object count, or a run of 0.
  expect(rule([2, 1, 3, 1, 4, 1, 2, 2, 1, 0])).toBe("ok");
  expect(rule([2, 1, 3, 1, 2, 1, 2, 0])).toBe("run");
  expect(rule([2, 1, 3, 1, 4, 1, 0, 2, 3, 0])).toBe("run");
  expect(rule([2, 1, 3, 0, 3, 0, 2, 1, 0, 9])).toBe("trailing bytes");
  // DELTA 4 → 5, two changes (idx 2 and 3), no areas.
  expect(rule([3, 4, 5, 2, 2, 1, 1, 3, 0])).toBe("ok");
  expect(rule([3, 5, 5, 0, 0])).toBe("seq");
  expect(rule([3, 4, 5, 2, 2, 1, 0, 3, 0])).toBe("not increasing");
  // A count the bytes left could not hold drives no allocation.
  expect(rule([3, 4, 5, 0xff, 0xff, 0x3f, 0])).toBe("count");
  expect(rule([2, 1, 0xc1, 0x84, 0x3d, 0, 0, 0])).toBe("count");
  // A seq past 2^53 the page could not hold exactly.
  expect(rule([2, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0])).toBe(
    "varint",
  );
});

test("a mangled chart is refused as unreadable, never with another error", () => {
  const r = random(11);
  const seeds = NAMES.map((n) => fixture(n).chart);
  for (let i = 0; i < 2_000; i++) {
    try {
      chartLabels(mutate(seeds[i % seeds.length] ?? new Uint8Array(), r));
    } catch (e) {
      expect(e).toBeInstanceOf(ChartError);
    }
  }
});

test("the poller's steps: snapshot, delta, nothing new, gap, too old, restart, bad bytes", () => {
  const f = fixture("full");
  const objects = f.expected.objects;
  const { from, to } = f.expected.delta;
  const snapshot = { status: 200 as const, bytes: f.snapshot };
  const delta = { status: 200 as const, bytes: f.delta };

  // First load: no seq yet, ask from 0; the snapshot replaces everything.
  expect(nextAfter(null, null)).toBe(0);
  expect(step(null, objects, snapshot)).toMatchObject({ kind: "snapshot", seq: from });
  // Staying current: the delta from the seq held moves to its `to`.
  expect(nextAfter(from, "snapshot")).toBe(from);
  expect(step(from, objects, delta)).toMatchObject({ kind: "delta", to });
  expect(step(to, objects, { status: 204 })).toEqual({ kind: "same" });
  // A delta from another seq is a gap: nothing applied, ask from 0.
  expect(step(to, objects, delta)).toEqual({ kind: "gap" });
  expect(step(null, objects, delta)).toEqual({ kind: "gap" });
  expect(nextAfter(to, "gap")).toBe(0);
  // Too old, or a restart: the server answers a snapshot to any after_seq.
  expect(step(to + 100, objects, snapshot)).toMatchObject({ kind: "snapshot", seq: from });
  // Bad bytes and another chart's object count: refused, ask from 0.
  expect(step(from, objects, { status: 200, bytes: f.delta.subarray(0, 3) })).toEqual({
    kind: "bad",
    rule: "truncated",
  });
  expect(step(from, objects + 1, snapshot)).toEqual({ kind: "bad", rule: "object count" });
  expect(step(from, 2, delta)).toEqual({ kind: "bad", rule: "idx" });
  expect(nextAfter(from, "bad")).toBe(0);
  // A frame type the route does not send is skipped.
  expect(step(from, objects, { status: 200, bytes: Uint8Array.from([6, 1]) })).toEqual({
    kind: "skip",
  });
  expect(nextAfter(from, "skip")).toBe(from);
});
