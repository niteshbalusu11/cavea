// The headless render (docs/04-renderer.md §26, REN-045), in the browser:
// given a chart, a state, a camera, a size, a pixel ratio and a theme, the
// pixels the shipped WASM draws with WebGL2. It drives the engine through the
// same `View` the SDK uses, with the SDK's own text rasteriser and atlas
// loader, and a virtual clock: frames are run back to back at fixed 60 Hz
// times until the engine asks for none and no text request is left, all in
// one task, then the drawing buffer is read before the browser presents it.
// So nothing depends on wall-clock time, on animation (reduced motion: every
// camera move is instant) or on how many frames came before the read.
import init, { View } from "../../packages/renderer/wasm/cavea_web.js";
import { loadGlyphAtlas } from "../../packages/renderer/src/atlas.ts";
import { takeEvents, type RendererEvent } from "../../packages/renderer/src/events.ts";
import { createRasteriser } from "../../packages/renderer/src/text.ts";

/** One picture: everything that decides it. */
export interface Panel {
  /** A chart the harness compiled, served at `charts/<chart>.ccf`. */
  chart: string;
  /** CSS pixels. */
  size: [number, number];
  /** Device pixel ratio; default 1. */
  dpr?: number;
  theme?: "light" | "dark" | "high-contrast";
  /** Theme tokens over the built-in theme, by name. */
  tokens?: Record<string, string>;
  /** Chart rectangle to show, `[x, y, width, height]` in chart units; default the whole chart. */
  camera?: [number, number, number, number];
  /** Unavailable seats: `[first idx, count]` runs; everything else is available. */
  unavailable?: Array<[number, number]>;
  /** Seats selected and held (the hold confirmed). */
  held?: string[];
  /** Seats selected with their hold still in flight. */
  pending?: string[];
  /** A mouse resting here, in CSS pixels: the hover ring. */
  hover?: [number, number];
  /** Keys pressed with the keyboard focus shown: the focus ring. */
  keys?: string[];
}

export interface Rendered {
  width: number;
  height: number;
  /** RGBA rows top to bottom, base64. */
  rgba: string;
  frames: number;
}

/** A settle that has not ended after this many frames has failed: something keeps animating. */
const MAX_FRAMES = 600;
const FRAME_MS = 1000 / 60;

let ready: Promise<Uint8Array> | undefined;
const charts = new Map<string, Promise<Uint8Array>>();

async function bytes(url: string): Promise<Uint8Array> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`${url}: ${response.status}`);
  }
  return new Uint8Array(await response.arrayBuffer());
}

async function render(panel: Panel): Promise<Rendered> {
  ready ??= init({ module_or_path: "wasm/cavea_web_bg.wasm" }).then(() =>
    loadGlyphAtlas("glyphs.png"),
  );
  const atlas = await ready;
  let chart = charts.get(panel.chart);
  if (chart === undefined) {
    chart = bytes(`charts/${panel.chart}.ccf`);
    charts.set(panel.chart, chart);
  }
  const ccf = await chart;

  // From here on, one task: the drawing buffer is not presented (and so not
  // cleared) before it is read.
  const canvas = document.createElement("canvas");
  document.body.replaceChildren(canvas);
  const view = new View(canvas);
  const rasteriser = createRasteriser();
  const log: RendererEvent[] = [];
  try {
    const [width, height] = panel.size;
    const dpr = panel.dpr ?? 1;
    let time = 0;
    let frames = 0;
    /** Runs one frame and answers the text it asked for; whether more frames are wanted. */
    const frame = (): boolean => {
      const wanted = view.frame(time);
      time += FRAME_MS;
      frames++;
      return drain() || wanted || view.needFrame;
    };
    const drain = (): boolean => {
      let text = false;
      for (const event of takeEvents(view)) {
        if (event.type === "textRequest") {
          view.setText(event.id, ...rasteriser.draw(event));
          text = true;
        } else {
          log.push(event);
        }
      }
      return text;
    };
    view.setReducedMotion(true);
    view.resize(width, height, dpr, 4);
    const tokens = Object.entries(panel.tokens ?? {}).flat();
    view.setTheme(panel.theme ?? "light", tokens);
    view.loadChart(ccf);
    view.setGlyphAtlas(atlas);
    drain();
    const codes = new Uint8Array(chartObjects(log));
    for (const [first, count] of panel.unavailable ?? []) {
      codes.fill(1, first, first + count);
    }
    view.setSnapshot(1, codes);
    view.flyTo(panel.camera === undefined ? undefined : [...panel.camera], undefined, false, true);
    frame();
    if (panel.held !== undefined) {
      view.select(panel.held);
      frame();
      const request = log.findLast((e) => e.type === "holdRequest");
      if (request === undefined || request.type !== "holdRequest") {
        throw new Error("no hold request for the held seats");
      }
      view.holdOk(request.request);
    }
    if (panel.pending !== undefined) {
      view.select(panel.pending);
    }
    if (panel.hover !== undefined) {
      view.pointer(1, 1, 0, panel.hover[0], panel.hover[1], time);
    }
    if (panel.keys !== undefined) {
      view.setFocusShown(true);
      for (const key of panel.keys) {
        view.key(key, 0);
        frame();
      }
    }
    while (frame()) {
      if (frames > MAX_FRAMES) {
        throw new Error(`still drawing after ${MAX_FRAMES} frames`);
      }
    }
    const errors = log.filter((e) => e.type === "error");
    if (errors.length > 0) {
      throw new Error(`engine errors: ${JSON.stringify(errors)}`);
    }
    const gl = canvas.getContext("webgl2");
    if (gl === null) {
      throw new Error("no WebGL2 context");
    }
    const w = canvas.width;
    const h = canvas.height;
    const pixels = new Uint8Array(w * h * 4);
    gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    if (gl.getError() !== gl.NO_ERROR) {
      throw new Error("readPixels failed");
    }
    // GL rows run bottom to top.
    const rgba = new Uint8Array(pixels.length);
    for (let y = 0; y < h; y++) {
      rgba.set(pixels.subarray((h - 1 - y) * w * 4, (h - y) * w * 4), y * w * 4);
    }
    return { width: w, height: h, rgba: base64(rgba), frames };
  } finally {
    rasteriser.release();
    view.free();
  }
}

/** The loaded chart's object count, from its `ready` event. */
function chartObjects(log: RendererEvent[]): number {
  const ready = log.find((e) => e.type === "ready");
  if (ready === undefined || ready.type !== "ready") {
    throw new Error("the chart did not load");
  }
  return ready.objects;
}

function base64(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(text);
}

Object.assign(window, { render });
