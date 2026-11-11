// Draws synthetic stadium seats with the Rust renderer, to see and measure it.
// Query parameters: `seats` (default 100000), `seed` (default 1), `shape`
// (0 circle, 1 rounded square, 2 chair, 3 diamond), `camera=x:y:width:height`
// in chart units, and `mark=start:count` to set that range of seats to the
// "unavailable" visual state after the first frame, which uploads only that
// dynamic sub-range.
import { loadRenderer, View } from "@cavea/renderer";

/** Exports present only in `cargo xtask build-web --demo` builds. */
interface DemoExports {
  loadSynthetic(seats: number, seed: number, shape: number): void;
  setCamera(x: number, y: number, width: number, height: number): void;
  setVisual(start: number, count: number, visual: number): void;
}

const VISUAL_UNAVAILABLE = 1;

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
  if (lines.length > 8) {
    lines.splice(4, lines.length - 8);
  }
  status.textContent = lines.join("\n");
  console.log(`[cavea-demo] ${line}`);
}

const params = new URLSearchParams(location.search);
const seats = Number(params.get("seats") ?? 100_000);
const seed = Number(params.get("seed") ?? 1);
const shape = Number(params.get("shape") ?? 0);
const mark = params.get("mark")?.split(":").map(Number);
const camera = params.get("camera")?.split(":").map(Number);

const renderer = await loadRenderer();
const view = new View(canvas) as View & Partial<DemoExports>;
const gl = canvas.getContext("webgl2");
const debug = gl?.getExtension("WEBGL_debug_renderer_info");
report(
  `renderer ${renderer.version()}; GPU: ${
    debug && gl ? String(gl.getParameter(debug.UNMASKED_RENDERER_WEBGL)) : "unknown"
  }`,
);

let scheduled = false;
let frames = 0;
let afterFirstFrame: (() => void) | undefined;

function schedule(): void {
  if (!scheduled) {
    scheduled = true;
    requestAnimationFrame(draw);
  }
}

function draw(time: number): void {
  scheduled = false;
  const start = performance.now();
  const again = view.frame(time);
  frames += 1;
  const error = gl?.getError() ?? -1;
  report(
    `frame ${frames}: ${(performance.now() - start).toFixed(2)} ms CPU; gl.getError() = ${error}`,
  );
  if (again) {
    schedule();
  }
  const next = afterFirstFrame;
  afterFirstFrame = undefined;
  next?.();
}

function resize(): void {
  view.resize(canvas.clientWidth, canvas.clientHeight, devicePixelRatio);
  schedule();
}

if (
  view.loadSynthetic === undefined ||
  view.setVisual === undefined ||
  view.setCamera === undefined
) {
  report("This WASM has no demo exports. Run `cargo xtask build-web --demo` and reload.");
} else {
  const setVisual = view.setVisual.bind(view);
  const start = performance.now();
  view.loadSynthetic(seats, seed, shape);
  report(`loaded ${seats} seats in ${(performance.now() - start).toFixed(1)} ms`);
  if (camera !== undefined && camera.length === 4) {
    const [x = 0, y = 0, width = 1, height = 1] = camera;
    view.setCamera(x, y, width, height);
  }
  if (mark !== undefined && mark.length === 2) {
    const [from = 0, count = 0] = mark;
    afterFirstFrame = () => {
      setVisual(from, count, VISUAL_UNAVAILABLE);
      report(`set seats ${from}..${from + count} unavailable`);
      schedule();
    };
  }
  new ResizeObserver(resize).observe(canvas);
}
