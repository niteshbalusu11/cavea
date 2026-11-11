// Draws a synthetic stadium, compiled in the browser, with the Rust renderer,
// to see and measure it. Query parameters: `seats` (default 100000), `seed`
// (default 1), `shape` (0 circle, 1 rounded, 2 chair), `theme`
// (light, dark, high-contrast), `camera=x:y:width:height` in chart units,
// `sold` (fraction of seats made unavailable after the first frame, in runs;
// default 0.3) or `mark=start:count` to make exactly that range unavailable.
// Status changes upload only the dynamic sub-range they touch. Pan and zoom
// with mouse, wheel, trackpad or touch; `gestures=greedy|cooperative|none`
// sets gesture handling (cooperative makes the page scroll around the map),
// `reducedMotion=1` turns animation off. `window.caveaDemo` counts frames and
// WebGL errors and offers `setTheme(name)` for scripted checks.
import { attach, loadRenderer, View, type GestureHandling } from "@cavea/renderer";

/** Exports present only in `cargo xtask build-web --demo` builds. */
interface DemoExports {
  setCamera(x: number, y: number, width: number, height: number): void;
  setStatus(start: number, count: number, status: number): void;
}
interface DemoStatics {
  demoChart(seats: number, seed: number, shape: number): Uint8Array;
}

type Theme = "light" | "dark" | "high-contrast";
const UNAVAILABLE = 1;

function element<T extends HTMLElement>(id: string, type: new () => T): T {
  const node = document.getElementById(id);
  if (!(node instanceof type)) {
    throw new Error(`#${id} is missing from index.html`);
  }
  return node;
}

const status = element("status", HTMLOutputElement);
const canvas = element("seats", HTMLCanvasElement);

const lines: string[] = [];
function report(line: string): void {
  lines.push(line);
  // Keep the first lines (load results) and the most recent frame.
  if (lines.length > 9) {
    lines.splice(5, lines.length - 9);
  }
  status.textContent = lines.join("\n");
  console.log(`[cavea-demo] ${line}`);
}

const params = new URLSearchParams(location.search);
const seats = Number(params.get("seats") ?? 100_000);
const seed = Number(params.get("seed") ?? 1);
const shape = Number(params.get("shape") ?? 0);
const sold = Number(params.get("sold") ?? 0.3);
const mark = params.get("mark")?.split(":").map(Number);
const camera = params.get("camera")?.split(":").map(Number);
const themeParam = params.get("theme");
const theme: Theme = themeParam === "dark" || themeParam === "high-contrast" ? themeParam : "light";
const gesturesParam = params.get("gestures");
const gestures: GestureHandling =
  gesturesParam === "cooperative" || gesturesParam === "none" ? gesturesParam : "greedy";
const reducedMotion = params.has("reducedMotion") ? params.get("reducedMotion") === "1" : undefined;
if (gestures === "cooperative") {
  document.body.classList.add("scrolling");
}

const renderer = await loadRenderer();
const view = new View(canvas) as View & Partial<DemoExports>;
const statics = View as typeof View & Partial<DemoStatics>;
const gl = canvas.getContext("webgl2");
const debug = gl?.getExtension("WEBGL_debug_renderer_info");
report(
  `renderer ${renderer.version()}; GPU: ${
    debug && gl ? String(gl.getParameter(debug.UNMASKED_RENDERER_WEBGL)) : "unknown"
  }`,
);

let host: ReturnType<typeof attach> | undefined;
function setTheme(name: Theme): void {
  view.setTheme(name);
  document.body.dataset["theme"] = name;
  host?.update();
}
const stats = { frames: 0, glErrors: 0, setTheme };
Object.assign(window, { caveaDemo: stats });
let afterFirstFrame: (() => void) | undefined;
let loadedAt = 0;

function onFrame(time: number): void {
  stats.frames += 1;
  const error = gl?.getError() ?? -1;
  if (error !== 0) {
    stats.glErrors += 1;
  }
  if (stats.frames === 1) {
    report(`first frame ${(performance.now() - loadedAt).toFixed(1)} ms after loadChart returned`);
  }
  report(`frame ${stats.frames} at ${time.toFixed(0)}: gl.getError() = ${error}`);
  const next = afterFirstFrame;
  afterFirstFrame = undefined;
  next?.();
}

/** Seeded runs of unavailable seats covering about `fraction` of the chart. */
function soldRuns(fraction: number, set: (start: number, count: number) => void): number {
  let state = seed >>> 0 || 1;
  const random = (): number => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) / 2 ** 32;
  };
  let runs = 0;
  for (let at = 0; at < seats;) {
    const count = 1 + Math.floor(random() * 24);
    const gap = Math.floor((random() * 2 * count * (1 - fraction)) / Math.max(fraction, 1e-3));
    at += gap;
    if (at >= seats) {
      break;
    }
    set(at, Math.min(count, seats - at));
    at += count;
    runs += 1;
  }
  return runs;
}

if (
  statics.demoChart === undefined ||
  view.setStatus === undefined ||
  view.setCamera === undefined
) {
  report("This WASM has no demo exports. Run `cargo xtask build-web --demo` and reload.");
} else {
  const setStatus = view.setStatus.bind(view);
  const compileStart = performance.now();
  const ccf = statics.demoChart(seats, seed, shape);
  const loadStart = performance.now();
  setTheme(theme);
  view.loadChart(ccf);
  loadedAt = performance.now();
  report(
    `compiled ${seats} seats in ${(loadStart - compileStart).toFixed(1)} ms (${ccf.length} B CCF); ` +
      `loadChart ${(loadedAt - loadStart).toFixed(1)} ms; theme ${theme}`,
  );
  if (camera !== undefined && camera.length === 4) {
    const [x = 0, y = 0, width = 1, height = 1] = camera;
    view.setCamera(x, y, width, height);
  }
  afterFirstFrame = () => {
    const start = performance.now();
    if (mark !== undefined && mark.length === 2) {
      const [from = 0, count = 0] = mark;
      setStatus(from, count, UNAVAILABLE);
      report(`set seats ${from}..${from + count} unavailable`);
    } else if (sold > 0) {
      const runs = soldRuns(sold, (from, count) => setStatus(from, count, UNAVAILABLE));
      report(`set ${runs} runs unavailable in ${(performance.now() - start).toFixed(1)} ms`);
    }
    host?.update();
  };
  const options = { gestureHandling: gestures, onFrame };
  host = attach(
    view,
    canvas,
    reducedMotion === undefined ? options : { ...options, reducedMotion },
  );
  report(`gesture handling: ${gestures}`);
}
