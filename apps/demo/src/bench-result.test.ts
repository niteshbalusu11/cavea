// The collector's only defence is this check: a result the page makes passes,
// and anything with a key, type or size the shape does not allow is refused.
import { expect, test } from "bun:test";
import { problem, type BenchResult } from "./bench-result.ts";

const dist = { median: 16.7, p95: 33.3, p99: 50, max: 400 };
const across = { median: 1, p90: 2 };
const result: BenchResult = {
  schema: 1,
  commit: "0ed2fef",
  build: "production",
  label: "phone",
  startedAt: "2026-11-11T21:44:09.266Z",
  environment: {
    userAgent: "Mozilla/5.0",
    browser: "Chrome 155",
    os: "Android 15",
    model: "Pixel 7a",
    pixelRatio: 2.625,
    viewport: [412, 915],
    cores: 8,
    memoryGb: null,
    battery: { level: 0.8, charging: false },
  },
  chart: { preset: "stadium-100k", seats: 100_000 },
  runs: 1,
  overBudgetMs: 20,
  s1: {
    runs: [
      {
        chartReadyMs: 44.8,
        marks: { state_ready: 44.8 },
        payload: [{ name: "a.wasm", transfer: 1, encoded: 2 }],
      },
    ],
    chartReadyMs: across,
    payloadBytes: 2,
  },
  s2: {
    runs: [{ frames: 1600, frameMs: dist, overBudget: 0.05, workMs: { p95: 3, max: 9 } }],
    p95: across,
    p99: across,
    overBudget: across,
  },
  s4: { runs: [{ taps: 50, selected: 50, inputToFrameMs: dist }], median: across, p95: across },
};

test("accepts a result and refuses anything else", () => {
  expect(problem(result)).toBeUndefined();
  expect(problem(JSON.parse(JSON.stringify(result)))).toBeUndefined();
  const changed = (change: (r: Record<string, unknown>) => void): string | undefined => {
    const copy = JSON.parse(JSON.stringify(result)) as Record<string, unknown>;
    change(copy);
    return problem(copy);
  };
  expect(changed((r) => (r["extra"] = 1))).toBe("$: unknown key");
  expect(changed((r) => (r["runs"] = "10"))).toBe("$.runs: not a number");
  expect(changed((r) => (r["label"] = "x".repeat(513)))).toContain("$.label");
  expect(changed((r) => (r["schema"] = 2))).toContain("$.schema");
  expect(changed((r) => delete r["commit"])).toContain("$.commit");
  expect(
    changed((r) => ((r["environment"] as Record<string, unknown>)["viewport"] = [1, 2, 3])),
  ).toContain("viewport");
  expect(
    changed((r) => ((r["s2"] as { runs: unknown[] }).runs = Array(101).fill(result.s2?.runs[0]))),
  ).toContain("$.s2.runs");
  const marks = Object.fromEntries(Array.from({ length: 17 }, (_, i) => [`m${i}`, i]));
  expect(
    changed(
      (r) => ((r["s1"] as { runs: Array<Record<string, unknown>> }).runs[0]!["marks"] = marks),
    ),
  ).toContain("keys");
  expect(problem(null)).toBe("$: not an object");
  expect(problem([result])).toBe("$: not an object");
});
