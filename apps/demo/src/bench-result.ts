// The benchmark page's result (docs/13-testing-benchmarks.md §6.2): one JSON
// document per visit, posted to the collector (bench/collect.ts), which
// accepts this shape and nothing else. Shared by both, so they cannot drift.

/** Nearest-rank percentiles of one run's samples, in ms. */
export interface Distribution {
  median: number;
  p95: number;
  p99: number;
  max: number;
}

/** Median and 90th percentile of a figure over the runs (13 §6.2: "report the median and the 90th percentile"). */
export interface AcrossRuns {
  median: number;
  p90: number;
}

/** S2 and S3: frames of the 30-second trace. */
export interface FrameRun {
  frames: number;
  /** Time between consecutive animation frames as the page saw them. */
  frameMs: Distribution;
  /** Share of frames whose interval was over `overBudgetMs`. */
  overBudget: number;
  /** The SDK's own measure of its work per frame (`onMetrics`): worst windows of the run. */
  workMs: { p95: number; max: number };
}

export interface BenchResult {
  schema: 1;
  commit: string;
  build: "production" | "development";
  /** Free text from `?label=`, such as the device's name. */
  label: string;
  startedAt: string;
  environment: {
    userAgent: string;
    browser: string;
    os: string;
    /** Where the browser says (Chromium's client hints); empty otherwise. */
    model: string;
    pixelRatio: number;
    viewport: [number, number];
    cores: number;
    memoryGb: number | null;
    battery: { level: number; charging: boolean } | null;
  };
  chart: { preset: string; seats: number };
  runs: number;
  overBudgetMs: number;
  s1?: {
    runs: Array<{
      chartReadyMs: number;
      marks: Record<string, number>;
      /** Resources the map fetched: file name, bytes over the network (0 from cache) and encoded size. */
      payload: Array<{ name: string; transfer: number; encoded: number }>;
    }>;
    chartReadyMs: AcrossRuns;
    payloadBytes: number;
  };
  s2?: { runs: FrameRun[]; p95: AcrossRuns; p99: AcrossRuns; overBudget: AcrossRuns };
  s3?: {
    runs: Array<FrameRun & { changes: number; deltaMs: Distribution }>;
    p95: AcrossRuns;
    overBudget: AcrossRuns;
    deltaP95: AcrossRuns;
  };
  s4?: {
    runs: Array<{ taps: number; selected: number; inputToFrameMs: Distribution }>;
    median: AcrossRuns;
    p95: AcrossRuns;
  };
  s5?: {
    runs: Array<{ wasmBytes: number; gpuBytes: number; jsHeapBytes: number | null }>;
    wasmBytes: AcrossRuns;
    gpuBytes: AcrossRuns;
  };
}

// A strict shape check: every key known, every value its type, strings and
// arrays bounded. Written for the collector, which trusts nothing it receives.
type Shape =
  | "number"
  | "string"
  | "boolean"
  | "null"
  | { literal: string | number }
  | { array: Shape; max: number; length?: number }
  | { record: Shape; max: number }
  | { object: Record<string, Shape>; optional?: string[] }
  | { or: Shape[] };

const STRING_MAX = 512;
const num: Shape = "number";
const dist: Shape = { object: { median: num, p95: num, p99: num, max: num } };
const across: Shape = { object: { median: num, p90: num } };
const frameRun: Record<string, Shape> = {
  frames: num,
  frameMs: dist,
  overBudget: num,
  workMs: { object: { p95: num, max: num } },
};
const RUNS = 100;
const SHAPE: Shape = {
  object: {
    schema: { literal: 1 },
    commit: "string",
    build: { or: [{ literal: "production" }, { literal: "development" }] },
    label: "string",
    startedAt: "string",
    environment: {
      object: {
        userAgent: "string",
        browser: "string",
        os: "string",
        model: "string",
        pixelRatio: num,
        viewport: { array: num, max: 2, length: 2 },
        cores: num,
        memoryGb: { or: [num, "null"] },
        battery: { or: ["null", { object: { level: num, charging: "boolean" } }] },
      },
    },
    chart: { object: { preset: "string", seats: num } },
    runs: num,
    overBudgetMs: num,
    s1: {
      object: {
        runs: {
          array: {
            object: {
              chartReadyMs: num,
              marks: { record: num, max: 16 },
              payload: {
                array: { object: { name: "string", transfer: num, encoded: num } },
                max: 64,
              },
            },
          },
          max: RUNS,
        },
        chartReadyMs: across,
        payloadBytes: num,
      },
    },
    s2: {
      object: {
        runs: { array: { object: frameRun }, max: RUNS },
        p95: across,
        p99: across,
        overBudget: across,
      },
    },
    s3: {
      object: {
        runs: { array: { object: { ...frameRun, changes: num, deltaMs: dist } }, max: RUNS },
        p95: across,
        overBudget: across,
        deltaP95: across,
      },
    },
    s4: {
      object: {
        runs: {
          array: { object: { taps: num, selected: num, inputToFrameMs: dist } },
          max: RUNS,
        },
        median: across,
        p95: across,
      },
    },
    s5: {
      object: {
        runs: {
          array: { object: { wasmBytes: num, gpuBytes: num, jsHeapBytes: { or: [num, "null"] } } },
          max: RUNS,
        },
        wasmBytes: across,
        gpuBytes: across,
      },
    },
  },
  optional: ["s1", "s2", "s3", "s4", "s5"],
};

/** The first way `value` is not a `BenchResult`, as a path and a reason; `undefined` if it is one. */
export function problem(value: unknown, shape: Shape = SHAPE, path = "$"): string | undefined {
  if (shape === "number") {
    return typeof value === "number" && Number.isFinite(value)
      ? undefined
      : `${path}: not a number`;
  }
  if (shape === "string") {
    return typeof value === "string" && value.length <= STRING_MAX
      ? undefined
      : `${path}: not a string of at most ${STRING_MAX} characters`;
  }
  if (shape === "boolean") {
    return typeof value === "boolean" ? undefined : `${path}: not a boolean`;
  }
  if (shape === "null") {
    return value === null ? undefined : `${path}: not null`;
  }
  if ("literal" in shape) {
    return value === shape.literal ? undefined : `${path}: not ${JSON.stringify(shape.literal)}`;
  }
  if ("or" in shape) {
    return shape.or.some((s) => problem(value, s, path) === undefined)
      ? undefined
      : `${path}: none of the allowed types`;
  }
  if ("array" in shape) {
    if (!Array.isArray(value) || value.length > shape.max) {
      return `${path}: not an array of at most ${shape.max}`;
    }
    if (shape.length !== undefined && value.length !== shape.length) {
      return `${path}: not ${shape.length} long`;
    }
    for (const [i, item] of value.entries()) {
      const found = problem(item, shape.array, `${path}[${i}]`);
      if (found !== undefined) {
        return found;
      }
    }
    return undefined;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return `${path}: not an object`;
  }
  const entries = Object.entries(value);
  if ("record" in shape) {
    if (entries.length > shape.max) {
      return `${path}: more than ${shape.max} keys`;
    }
    for (const [key, item] of entries) {
      const found =
        // The path never quotes a key it received.
        key.length > 64
          ? `${path}: a key over 64 characters`
          : problem(item, shape.record, `${path}.*`);
      if (found !== undefined) {
        return found;
      }
    }
    return undefined;
  }
  for (const [key] of entries) {
    if (!Object.hasOwn(shape.object, key)) {
      return `${path}: unknown key`;
    }
  }
  for (const [key, inner] of Object.entries(shape.object)) {
    const item = (value as Record<string, unknown>)[key];
    if (item === undefined && shape.optional?.includes(key) === true) {
      continue;
    }
    const found = problem(item, inner, `${path}.${key}`);
    if (found !== undefined) {
      return found;
    }
  }
  return undefined;
}
