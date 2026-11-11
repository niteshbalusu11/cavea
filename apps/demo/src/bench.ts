// The benchmark page (docs/13-testing-benchmarks.md §6.2, TST-012): scenarios
// S1 to S5 on a synthetic preset through the public SDK, ten runs each, and
// one JSON result posted to the collector (`bun bench/collect.ts`). Needs the
// `--demo` WASM (the chart is compiled in a worker). It runs on its own once
// started, so a phone on the local network can run it: no DevTools protocol.
//
// Query parameters: `preset` (default stadium-100k), `runs` (default 10),
// `only=S1,S4` (a subset), `collector=http://host:port` (where to post the
// result; none keeps it on the page), `label` (free text saved with the
// result, such as the device's name), `auto=1` (start without the button),
// `budget=ms` (a frame interval over this counts as over budget; default 20,
// a missed frame at 60 Hz).
//
// S1: cold load to chart-ready, each run in a fresh frame of
// bench-load.html, which holds the SDK and nothing else (a new WASM instance;
// the browser's HTTP cache is warm after the first run, and the page cannot
// clear it). S2: the 30-second gesture trace of
// bench/traces/s2-gestures.json replayed with real time, scaled to the map's
// size, as synthetic pointer events on the canvas. S3: S2 while 1,000 seat
// states change a second. S4: 50 taps on seats, each to the next frame. S5:
// memory after S2: WASM memory, an estimate of GPU memory from the WebGL calls,
// the JavaScript heap where the browser reports it.
import {
  createSeatMap,
  customState,
  type AdapterContext,
  type Metrics,
  type SeatMap,
} from "@cavea/renderer";
import trace from "../../../bench/traces/s2-gestures.json";
import init from "../../../packages/renderer/wasm/cavea_web.js";
import type { AcrossRuns, BenchResult, Distribution, FrameRun } from "./bench-result.ts";

/** The commit the page was built from (vite.config.ts). */
declare const CAVEA_COMMIT: string;

const params = new URLSearchParams(location.search);
const preset = params.get("preset") ?? "stadium-100k";
const runs = Math.max(1, Math.min(100, Number(params.get("runs") ?? 10) || 10));
const only = new Set((params.get("only") ?? "S1,S2,S3,S4,S5").toUpperCase().split(","));
const collector = params.get("collector");
const label = (params.get("label") ?? "").slice(0, 64);
const overBudgetMs = Number(params.get("budget") ?? 20) || 20;

const status = document.getElementById("status") as HTMLPreElement;
const container = document.getElementById("map") as HTMLDivElement;
const say = (line: string): void => {
  status.textContent = `${line}\n${status.textContent ?? ""}`.slice(0, 4000);
};
const frame = (): Promise<number> => new Promise((resolve) => requestAnimationFrame(resolve));
const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** Nearest-rank percentile of sorted `values`. */
const rank = (sorted: number[], p: number): number =>
  sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * sorted.length) - 1))] ?? 0;
const round = (v: number): number => Math.round(v * 1000) / 1000;
function distribution(values: number[]): Distribution {
  const sorted = [...values];
  // oxlint-disable-next-line unicorn/no-array-sort -- sorts its own copy; `toSorted` is ES2023, past the lib (ES2022).
  sorted.sort((a, b) => a - b);
  return {
    median: round(rank(sorted, 0.5)),
    p95: round(rank(sorted, 0.95)),
    p99: round(rank(sorted, 0.99)),
    max: round(sorted.at(-1) ?? 0),
  };
}
function across(values: number[]): AcrossRuns {
  const sorted = [...values];
  // oxlint-disable-next-line unicorn/no-array-sort -- sorts its own copy; `toSorted` is ES2023, past the lib (ES2022).
  sorted.sort((a, b) => a - b);
  return { median: round(rank(sorted, 0.5)), p90: round(rank(sorted, 0.9)) };
}

// GPU memory, estimated from the WebGL calls the map makes: buffer and
// texture sizes as allocated, plus the drawing buffer. The driver's own
// overhead is not counted. Installed before the map makes its context.
const gpu = { buffers: new Map<WebGLBuffer, number>(), textures: new Map<WebGLTexture, number>() };
let glCanvas: HTMLCanvasElement | undefined;
const gpuBytes = (): number =>
  [...gpu.buffers.values(), ...gpu.textures.values()].reduce((a, b) => a + b, 0) +
  (glCanvas === undefined ? 0 : glCanvas.width * glCanvas.height * 4);
function tally(gl: WebGL2RenderingContext, canvas: HTMLCanvasElement): void {
  glCanvas = canvas;
  const bound = new Map<number, WebGLBuffer | null>();
  const units = new Map<number, WebGLTexture | null>();
  let unit: number = gl.TEXTURE0;
  const texel = (format: number): number =>
    format === gl.R8 || format === gl.RED || format === gl.ALPHA || format === gl.LUMINANCE
      ? 1
      : format === gl.RG8 || format === gl.RG
        ? 2
        : format === gl.RGB8 || format === gl.RGB
          ? 3
          : 4;
  const { bindBuffer, bufferData, deleteBuffer, activeTexture, bindTexture } = gl;
  const { texImage2D, texStorage2D, deleteTexture } = gl;
  gl.bindBuffer = (target, buffer) => {
    bound.set(target, buffer);
    bindBuffer.call(gl, target, buffer);
  };
  gl.bufferData = ((target: number, data: number | BufferSource | null, ...rest: number[]) => {
    const buffer = bound.get(target);
    if (buffer !== null && buffer !== undefined) {
      gpu.buffers.set(buffer, typeof data === "number" ? data : (data?.byteLength ?? 0));
    }
    (bufferData as (...a: unknown[]) => void).call(gl, target, data, ...rest);
  }) as typeof gl.bufferData;
  gl.deleteBuffer = (buffer) => {
    if (buffer !== null) {
      gpu.buffers.delete(buffer);
    }
    deleteBuffer.call(gl, buffer);
  };
  gl.activeTexture = (texture) => {
    unit = texture;
    activeTexture.call(gl, texture);
  };
  gl.bindTexture = (target, texture) => {
    units.set(unit, texture);
    bindTexture.call(gl, target, texture);
  };
  gl.texImage2D = ((...args: unknown[]) => {
    const texture = units.get(unit);
    // Only the sized overload: (target, level, internal format, width, height, border, format, type, …).
    if (texture !== null && texture !== undefined && args.length >= 8 && args[1] === 0) {
      const [, , , w, h, , format] = args as number[];
      gpu.textures.set(texture, (w ?? 0) * (h ?? 0) * texel(format ?? 0));
    }
    (texImage2D as (...a: unknown[]) => void).apply(gl, args);
  }) as typeof gl.texImage2D;
  gl.texStorage2D = (target, levels, format, w, h) => {
    const texture = units.get(unit);
    if (texture !== null && texture !== undefined) {
      gpu.textures.set(texture, w * h * texel(format));
    }
    texStorage2D.call(gl, target, levels, format, w, h);
  };
  gl.deleteTexture = (texture) => {
    if (texture !== null) {
      gpu.textures.delete(texture);
    }
    deleteTexture.call(gl, texture);
  };
}
const getContext = HTMLCanvasElement.prototype.getContext as (
  this: HTMLCanvasElement,
  type: string,
  options?: unknown,
) => RenderingContext | null;
const tallied = new WeakSet<object>();
HTMLCanvasElement.prototype.getContext = function (
  this: HTMLCanvasElement,
  type: string,
  options?: unknown,
) {
  const context = getContext.call(this, type, options);
  if (type === "webgl2" && context !== null && !tallied.has(context)) {
    tallied.add(context);
    tally(context as WebGL2RenderingContext, this);
  }
  return context;
} as typeof HTMLCanvasElement.prototype.getContext;

async function main(): Promise<void> {
  say(`compiling ${preset}…`);
  const { ccf, labels } = await compile(preset);
  const startedAt = new Date().toISOString();
  const result: BenchResult = {
    schema: 1,
    commit: CAVEA_COMMIT,
    build: import.meta.env.DEV ? "development" : "production",
    label,
    startedAt,
    environment: await environment(),
    chart: { preset, seats: labels.length },
    runs,
    overBudgetMs,
  };

  if (only.has("S1")) {
    const s1: NonNullable<BenchResult["s1"]>["runs"] = [];
    for (let run = 0; run < runs; run++) {
      say(`S1 run ${run + 1} of ${runs}: cold load`);
      s1.push(await coldLoad(ccf));
    }
    result.s1 = {
      runs: s1,
      chartReadyMs: across(s1.map((r) => r.chartReadyMs)),
      payloadBytes: s1[0]?.payload.reduce((n, p) => n + p.encoded, 0) ?? 0,
    };
  }

  let ctx: AdapterContext | undefined;
  const windows: Metrics[] = [];
  let selections = 0;
  const map = await createSeatMap(container, {
    chart: { bytes: ccf },
    state: customState({
      connect(context) {
        ctx = context;
        return Promise.resolve({ kind: "map", unavailable: [] });
      },
      disconnect: () => undefined,
    }),
    selection: { max: 100 },
    reducedMotion: false,
    ui: { loading: false, tooltip: false },
    onMetrics: (m) => windows.push(m),
    onSelectionChange: () => selections++,
  });
  const canvas = container.shadowRoot?.querySelector("canvas");
  if (canvas === null || canvas === undefined) {
    throw new Error("no canvas in the map");
  }
  // Synthetic pointers are not active pointers, so they cannot be captured.
  canvas.setPointerCapture = () => undefined;

  const s2: FrameRun[] = [];
  const s3: Array<FrameRun & { changes: number; deltaMs: Distribution }> = [];
  const s5: NonNullable<BenchResult["s5"]>["runs"] = [];
  const s4: NonNullable<BenchResult["s4"]>["runs"] = [];
  const states = only.has("S3") ? { taken: new Set<string>() } : undefined;
  for (let run = 0; run < runs; run++) {
    if (only.has("S2") || only.has("S5")) {
      say(`S2 run ${run + 1} of ${runs}: the 30-second trace`);
      s2.push(await replay(map, canvas, windows));
      if (only.has("S5")) {
        // Loaded already, so this returns its exports without fetching.
        const exports = await init({ module_or_path: "" });
        const heap = (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory;
        s5.push({
          wasmBytes: exports.memory.buffer.byteLength,
          gpuBytes: gpuBytes(),
          jsHeapBytes: heap?.usedJSHeapSize ?? null,
        });
      }
    }
    if (states !== undefined && ctx !== undefined) {
      say(`S3 run ${run + 1} of ${runs}: the trace with 1,000 changes a second`);
      s3.push(await replayWithChanges(map, canvas, windows, ctx, labels, states.taken));
      ctx.applySnapshot({ kind: "map", unavailable: [] });
      states.taken.clear();
    }
    if (only.has("S4")) {
      say(`S4 run ${run + 1} of ${runs}: 50 taps`);
      s4.push(await taps(map, canvas, labels, () => selections));
    }
  }
  if (only.has("S2")) {
    result.s2 = {
      runs: s2,
      p95: across(s2.map((r) => r.frameMs.p95)),
      p99: across(s2.map((r) => r.frameMs.p99)),
      overBudget: across(s2.map((r) => r.overBudget)),
    };
  }
  if (s3.length > 0) {
    result.s3 = {
      runs: s3,
      p95: across(s3.map((r) => r.frameMs.p95)),
      overBudget: across(s3.map((r) => r.overBudget)),
      deltaP95: across(s3.map((r) => r.deltaMs.p95)),
    };
  }
  if (only.has("S4")) {
    result.s4 = {
      runs: s4,
      median: across(s4.map((r) => r.inputToFrameMs.median)),
      p95: across(s4.map((r) => r.inputToFrameMs.p95)),
    };
  }
  if (only.has("S5")) {
    result.s5 = {
      runs: s5,
      wasmBytes: across(s5.map((r) => r.wasmBytes)),
      gpuBytes: across(s5.map((r) => r.gpuBytes)),
    };
  }
  map.destroy();
  await report(result);
}

/** The preset's public CCF and seat labels, from a worker (bench-chart.ts). */
function compile(name: string): Promise<{ ccf: Uint8Array; labels: string[] }> {
  const worker = new Worker(new URL("./bench-chart.ts", import.meta.url), { type: "module" });
  return new Promise((resolve, reject) => {
    worker.addEventListener(
      "message",
      (e: MessageEvent<{ ccf: Uint8Array; labels: string[] } | { error: string }>) => {
        worker.terminate();
        if ("error" in e.data) {
          reject(new Error(e.data.error));
        } else {
          resolve(e.data);
        }
      },
    );
    worker.addEventListener("error", (e) => {
      worker.terminate();
      reject(new Error(e.message));
    });
    // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a worker's postMessage takes no origin
    worker.postMessage(name);
  });
}

async function environment(): Promise<BenchResult["environment"]> {
  type Hints = { brands?: Array<{ brand: string; version: string }>; platform?: string };
  type HighEntropy = Hints & {
    model?: string;
    platformVersion?: string;
    fullVersionList?: Hints["brands"];
  };
  const agent = (
    navigator as Navigator & {
      userAgentData?: Hints & { getHighEntropyValues(hints: string[]): Promise<HighEntropy> };
      deviceMemory?: number;
      getBattery?: () => Promise<{ level: number; charging: boolean }>;
    }
  ).userAgentData;
  const hints = await agent
    ?.getHighEntropyValues(["model", "platformVersion", "fullVersionList"])
    .catch(() => undefined);
  const brand = hints?.fullVersionList?.find((b) => !/Not.A.Brand|Chromium/.test(b.brand));
  const battery = await (
    navigator as Navigator & { getBattery?: () => Promise<{ level: number; charging: boolean }> }
  )
    .getBattery?.()
    .catch(() => undefined);
  const memory = (navigator as Navigator & { deviceMemory?: number }).deviceMemory;
  const ua = navigator.userAgent;
  return {
    userAgent: ua.slice(0, 512),
    browser: (brand === undefined
      ? (ua.match(/(Firefox|Version|Chrome)\/[\d.]+/)?.[0] ?? "")
      : `${brand.brand} ${brand.version}`
    ).slice(0, 120),
    os: (hints?.platform === undefined
      ? (ua.match(/\(([^)]*)\)/)?.[1] ?? "")
      : `${hints.platform} ${hints.platformVersion ?? ""}`.trim()
    ).slice(0, 120),
    model: (hints?.model ?? "").slice(0, 120),
    pixelRatio: devicePixelRatio,
    viewport: [container.clientWidth, container.clientHeight],
    cores: navigator.hardwareConcurrency,
    memoryGb: memory ?? null,
    battery: battery === undefined ? null : { level: battery.level, charging: battery.charging },
  };
}

/** One cold load in a fresh frame: chart-ready and what was fetched. */
async function coldLoad(ccf: Uint8Array): Promise<NonNullable<BenchResult["s1"]>["runs"][number]> {
  const child = document.createElement("iframe");
  child.style.cssText = `position:fixed;inset:0;width:100%;height:100%;border:0;background:#fff`;
  child.src = "bench-load.html";
  const reply = new Promise<{
    marks: Record<string, number>;
    payload: Array<{ name: string; transfer: number; encoded: number }>;
  }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("S1: no chart-ready within 60 s")), 60_000);
    const listen = (e: MessageEvent): void => {
      if (e.source !== child.contentWindow || e.origin !== location.origin) {
        return;
      }
      if (e.data === "ready") {
        child.contentWindow?.postMessage(ccf, location.origin);
        return;
      }
      clearTimeout(timer);
      removeEventListener("message", listen);
      resolve(e.data as never);
    };
    addEventListener("message", listen);
  });
  document.body.append(child);
  try {
    const { marks, payload } = await reply;
    return { chartReadyMs: round(marks["state_ready"] ?? -1), marks, payload };
  } finally {
    child.remove();
    await sleep(200);
  }
}

type Step = [number, string, ...Array<number | string>];

/**
 * Replays the S2 trace in real time from its starting camera, dispatching
 * each step at the first animation frame at or after its time, and measures
 * the interval between frames meanwhile. `each` runs every frame (S3's changes).
 */
async function replay(
  map: SeatMap,
  canvas: HTMLCanvasElement,
  windows: Metrics[],
  each?: (elapsed: number) => void,
): Promise<FrameRun> {
  const [x, y, width, height] = trace.camera;
  map.zoomTo({ x: x ?? 0, y: y ?? 0, width: width ?? 1, height: height ?? 1 }, { animate: false });
  // Let the camera move and the pictures settle before timing.
  for (let i = 0; i < 30; i++) {
    await frame();
  }
  windows.length = 0;
  const steps = trace.steps as unknown as Step[];
  const [vw = 1, vh = 1] = trace.viewport;
  const rect = canvas.getBoundingClientRect();
  const sx = rect.width / vw;
  const sy = rect.height / vh;
  const intervals: number[] = [];
  const start = await frame();
  let last = start;
  let next = 0;
  const end = (steps.at(-1)?.[0] ?? 0) + 500;
  for (let now = start; now - start < end; now = await frame()) {
    if (now !== start) {
      intervals.push(now - last);
    }
    last = now;
    each?.(now - start);
    for (; next < steps.length && (steps[next]?.[0] ?? 0) <= now - start; next++) {
      const [t, op, id, type, px, py] = steps[next] ?? [];
      const kind = {
        down: "pointerdown",
        move: "pointermove",
        up: "pointerup",
        cancel: "pointercancel",
      }[String(op)];
      if (kind !== undefined) {
        const event = new PointerEvent(kind, {
          pointerId: Number(id),
          pointerType: String(type),
          isPrimary: Number(id) === 1,
          clientX: rect.left + Number(px) * sx,
          clientY: rect.top + Number(py) * sy,
          button: kind === "pointermove" ? -1 : 0,
          buttons: kind === "pointerdown" || kind === "pointermove" ? 1 : 0,
          bubbles: true,
          cancelable: true,
        });
        // The step's own time on the page's clock, not the time it was
        // dispatched: a slow frame must not bunch the steps it held back,
        // which would change the release velocity and so the inertia.
        Object.defineProperty(event, "timeStamp", { value: start + Number(t) });
        canvas.dispatchEvent(event);
      }
    }
  }
  const frameMs = distribution(intervals);
  return {
    frames: intervals.length,
    frameMs,
    overBudget: round(
      intervals.filter((v) => v > overBudgetMs).length / Math.max(1, intervals.length),
    ),
    workMs: {
      p95: round(Math.max(0, ...windows.map((w) => w.frameMs.p95))),
      max: round(Math.max(0, ...windows.map((w) => w.frameMs.max))),
    },
  };
}

/** S3: the replay while 1,000 seat states change a second, through the adapter as a custom host sends them. */
async function replayWithChanges(
  map: SeatMap,
  canvas: HTMLCanvasElement,
  windows: Metrics[],
  ctx: AdapterContext,
  labels: string[],
  taken: Set<string>,
): Promise<FrameRun & { changes: number; deltaMs: Distribution }> {
  const applied: number[] = [];
  let changes = 0;
  let seed = 1;
  const random = (): number => {
    seed = (seed * 1_103_515_245 + 12_345) % 2 ** 31;
    return seed / 2 ** 31;
  };
  const frameRun = await replay(map, canvas, windows, (elapsed) => {
    const due = Math.floor(elapsed) - changes;
    if (due <= 0) {
      return;
    }
    const unavailable: string[] = [];
    const available: string[] = [];
    for (let i = 0; i < due; i++) {
      const seat = labels[Math.floor(random() * labels.length)] ?? "";
      if (taken.delete(seat)) {
        available.push(seat);
      } else {
        taken.add(seat);
        unavailable.push(seat);
      }
    }
    changes += due;
    const before = performance.now();
    ctx.applyDelta({ unavailable, available });
    applied.push(performance.now() - before);
  });
  return { ...frameRun, changes, deltaMs: distribution(applied) };
}

/** S4: 25 seats across the chart, each tapped on and off: from the release to the next frame. */
async function taps(
  map: SeatMap,
  canvas: HTMLCanvasElement,
  labels: string[],
  selections: () => number,
): Promise<{ taps: number; selected: number; inputToFrameMs: Distribution }> {
  const delays: number[] = [];
  const before = selections();
  for (let i = 0; i < 25; i++) {
    const seat = labels[Math.floor(((i + 0.5) * labels.length) / 25)] ?? "";
    map.zoomTo(seat, { animate: false });
    for (let f = 0; f < 10; f++) {
      await frame();
    }
    const rect = canvas.getBoundingClientRect();
    const at = { clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2 };
    for (let tap = 0; tap < 2; tap++) {
      const pointer = { pointerId: 1, pointerType: "mouse", isPrimary: true, bubbles: true, ...at };
      canvas.dispatchEvent(new PointerEvent("pointerdown", { ...pointer, button: 0, buttons: 1 }));
      await frame();
      const released = performance.now();
      canvas.dispatchEvent(new PointerEvent("pointerup", { ...pointer, button: 0, buttons: 0 }));
      // Requested after the map's own frame, so it runs after the map has drawn.
      await frame();
      delays.push(performance.now() - released);
      // Past the double-tap window (300 ms), so the second tap deselects instead of zooming.
      await sleep(400);
    }
  }
  return {
    taps: delays.length,
    selected: selections() - before,
    inputToFrameMs: distribution(delays),
  };
}

async function report(result: BenchResult): Promise<void> {
  const json = JSON.stringify(result);
  Object.assign(window, { benchResult: result });
  const out = document.getElementById("result") as HTMLPreElement;
  out.textContent = JSON.stringify(result, null, 2);
  if (collector === null) {
    say("done; no collector given, the result is below");
    return;
  }
  const response = await fetch(new URL("/results", collector), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: json,
  }).catch((error: unknown) => {
    say(`could not post to ${collector}: ${String(error)}`);
    return undefined;
  });
  say(
    response?.ok === true
      ? `posted to ${collector}`
      : `the collector refused it: ${response?.status ?? "no answer"}`,
  );
}

{
  const button = document.getElementById("start") as HTMLButtonElement;
  const go = (): void => {
    button.disabled = true;
    main().then(
      () => Object.assign(window, { benchDone: true }),
      (error: unknown) => {
        say(`failed: ${String(error)}`);
        Object.assign(window, { benchDone: true, benchError: String(error) });
      },
    );
  };
  button.addEventListener("click", go);
  if (params.get("auto") === "1") {
    go();
  }
}
