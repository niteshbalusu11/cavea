// Draws a synthetic stadium, compiled in the browser, with the Rust renderer,
// to see and measure it. Query parameters: `seats` (default 100000), `seed`
// (default 1), `shape` (0 circle, 1 rounded, 2 chair), `theme`
// (light, dark, high-contrast), `camera=x:y:width:height` in chart units,
// `sold` (fraction of seats unavailable in the first snapshot, in runs;
// default 0.3) or `mark=start:count` to make exactly that range unavailable.
// State reaches the view as realtime protocol frames (08 §8) built here. Pan and zoom
// with mouse, wheel, trackpad or touch; `gestures=greedy|cooperative|none`
// sets gesture handling (cooperative makes the page scroll around the map),
// `reducedMotion=1` turns animation off.
//
// Tap or click seats to select and deselect them. A fake host answers each
// hold request after `holdMs` (default 400) milliseconds, then marks the seats
// unavailable in a DELTA and lists them in a PRIVATE frame, as the server does;
// `fail=n` makes every n-th hold fail as taken by someone else, so the revert
// shows. `max` and `min` set the selection rules (default 4 and 0); `manual=1`
// selects without holds. Small seats zoom instead of selecting (touch below
// 18 px, mouse below 5 px).
//
// `window.caveaDemo` counts frames and WebGL errors, keeps every engine event
// in `events`, and offers `setTheme(name)`, `resnapshot()` and
// `take(label)` (someone else takes a seat, in a DELTA) and `gap()` (a DELTA
// that skips a sequence number, so the view asks to resync and this host
// answers with a SNAPSHOT) for scripted checks.
import {
  attach,
  loadRenderer,
  View,
  type GestureHandling,
  type RendererEvent,
} from "@cavea/renderer";

/** Exports present only in `cargo xtask build-web --demo` builds. */
interface DemoExports {
  setCamera(x: number, y: number, width: number, height: number): void;
  idxOf(label: string): number | undefined;
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
const selectionLine = element("selection", HTMLOutputElement);
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
const max = Number(params.get("max") ?? 4);
const min = Number(params.get("min") ?? 0);
const manual = params.get("manual") === "1";
const failEvery = Number(params.get("fail") ?? 0);
const holdMs = Number(params.get("holdMs") ?? 400);
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
const events: RendererEvent[] = [];
const stats = {
  frames: 0,
  glErrors: 0,
  setTheme,
  events,
  resnapshot: () => sendSnapshot(),
  take: (label: string) => sendDelta(new Map(idxOf([label]).map((i) => [i, 1]))),
  gap: () => sendDelta(new Map(), true),
};
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

let rngState = seed >>> 0 || 1;
function random(): number {
  rngState ^= rngState << 13;
  rngState ^= rngState >>> 17;
  rngState ^= rngState << 5;
  return (rngState >>> 0) / 2 ** 32;
}

/** Seeded runs of unavailable seats covering about `fraction` of the chart. */
function soldRuns(fraction: number, set: (start: number, count: number) => void): number {
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

/** Unsigned LEB128 (08 §8), for numbers up to 2^53. */
function varint(out: number[], value: number): void {
  let v = value;
  while (v >= 0x80) {
    out.push((v % 0x80) | 0x80);
    v = Math.floor(v / 0x80);
  }
  out.push(v);
}

/** A `list()` of increasing `idx`, each followed by `field` bytes, then no areas. */
function list(out: number[], idx: number[], field?: (i: number) => number): Uint8Array {
  varint(out, idx.length);
  let previous = 0;
  idx.forEach((value, i) => {
    varint(out, value - previous);
    previous = value;
    if (field !== undefined) {
      out.push(field(i));
    }
  });
  out.push(0);
  return Uint8Array.from(out);
}

/** Numbers in increasing order (a typed array sorts numerically). */
function sorted(values: Iterable<number>): Uint32Array {
  // oxlint-disable-next-line unicorn/no-array-sort -- sorts its own fresh copy; toSorted is ES2023
  return Uint32Array.from(values).sort();
}

/** The fake host: the public state, the buyer's holds, and late answers. */
const publicState = new Uint8Array(seats);
function fill(from: number, count: number): void {
  publicState.fill(UNAVAILABLE, from, from + count);
}
let seq = 0;
const held = new Set<number>();

function apply(frame: Uint8Array): void {
  try {
    view.applyFrame(frame);
  } catch (error) {
    note(`frame refused: ${String(error)}`);
  }
  host?.update();
}
function sendSnapshot(): void {
  const out = [0x02];
  varint(out, seq);
  varint(out, seats);
  out.push(0);
  varint(out, seats);
  apply(Uint8Array.from([...out, ...publicState, 0]));
}
/** Sets codes and sends them as one DELTA; `skip` leaves a sequence gap. */
function sendDelta(changes: Map<number, number>, skip = false): void {
  const idx = [...sorted(changes.keys())];
  idx.forEach((i) => (publicState[i] = changes.get(i) ?? 0));
  const out = [0x03];
  varint(out, skip ? seq + 1 : seq);
  seq += 1;
  varint(out, skip ? seq + 1 : seq);
  apply(list(out, idx, (i) => changes.get(idx[i] ?? 0) ?? 0));
}
function sendPrivate(): void {
  const out = [0x04];
  varint(out, Date.now() + 15 * 60_000);
  apply(list(out, [...sorted(held)]));
}
function idxOf(labels: string[]): number[] {
  return labels.flatMap((label) => view.idxOf?.(label) ?? []);
}
let selected: string[] = [];
let expiresAt: number | null = null;
let holds = 0;
let pending = 0;
let hover = "";
let ready = true;
let lastNote = "";
function showSelection(): void {
  const expiry = expiresAt === null ? "none" : new Date(expiresAt).toLocaleTimeString();
  selectionLine.textContent =
    `selected ${selected.length}/${max}: ${selected.join(" ")}\n` +
    `holds in flight: ${pending}; held: ${held.size}, expire ${expiry}; ` +
    `checkout ${ready ? "ready" : "not ready"}; hover: ${hover || "none"}\n${lastNote}`;
}
function note(line: string): void {
  lastNote = line;
  console.log(`[cavea-demo] ${line}`);
}
function onEvents(batch: RendererEvent[]): void {
  for (const event of batch) {
    events.push(event);
    switch (event.type) {
      case "selectionChanged":
        selected = event.selected.map((o) => o.label);
        expiresAt = event.expiresAt;
        note(`selection: ${selected.join(", ") || "empty"}`);
        break;
      case "holdRequest": {
        holds += 1;
        pending += 1;
        const fails = failEvery > 0 && holds % failEvery === 0;
        note(`hold ${event.request} requested: ${event.labels.join(", ")}`);
        setTimeout(() => {
          pending -= 1;
          const idx = idxOf(event.labels);
          if (fails) {
            note(`hold ${event.request} failed: ${event.labels.join(", ")} taken`);
            view.holdFailed(event.request, "object_not_free", []);
            sendDelta(new Map(idx.map((i) => [i, 1])));
          } else {
            note(`hold ${event.request} ok`);
            view.holdOk(event.request);
            idx.forEach((i) => held.add(i));
            sendDelta(new Map(idx.map((i) => [i, 1])));
            sendPrivate();
          }
          host?.update();
        }, holdMs);
        break;
      }
      case "releaseRequest": {
        note(`release ${event.request}: ${event.labels.join(", ")}`);
        const idx = idxOf(event.labels);
        idx.forEach((i) => held.delete(i));
        sendPrivate();
        sendDelta(new Map(idx.map((i) => [i, 0])));
        break;
      }
      case "selectionInvalid":
        if (event.rule === "min") {
          ready = false;
        }
        note(`refused (${event.rule}): ${event.objects.join(", ")}`);
        break;
      case "selectionValid":
        ready = true;
        break;
      case "objectUnavailable":
        note(`${event.label} was taken`);
        break;
      case "holdLost":
        note(`hold lost: ${event.labels.join(", ")}`);
        break;
      case "resync":
        note(`resync from seq ${String(event.seq)}: sending a snapshot at ${seq}`);
        setTimeout(sendSnapshot, 0);
        break;
      case "objectHover":
        hover = event.object?.label ?? "";
        break;
      default:
        break;
    }
  }
  showSelection();
}

if (statics.demoChart === undefined || view.idxOf === undefined || view.setCamera === undefined) {
  report("This WASM has no demo exports. Run `cargo xtask build-web --demo` and reload.");
} else {
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
  view.setRules(max, min);
  if (manual) {
    view.setManualSession(true);
  }
  if (camera !== undefined && camera.length === 4) {
    const [x = 0, y = 0, width = 1, height = 1] = camera;
    view.setCamera(x, y, width, height);
  }
  afterFirstFrame = () => {
    if (mark !== undefined && mark.length === 2) {
      const [from = 0, count = 0] = mark;
      fill(from, count);
      report(`seats ${from}..${from + count} unavailable`);
    } else if (sold > 0) {
      report(`${soldRuns(sold, fill)} runs unavailable`);
    }
    const start = performance.now();
    sendSnapshot();
    report(`snapshot at seq ${seq} applied in ${(performance.now() - start).toFixed(1)} ms`);
  };
  const options = { gestureHandling: gestures, onFrame, onEvents };
  host = attach(
    view,
    canvas,
    reducedMotion === undefined ? options : { ...options, reducedMotion },
  );
  report(`gesture handling: ${gestures}`);
}
