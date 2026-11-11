// The designer's canvas (docs/07-designer.md §2, docs/04-renderer.md §25):
// a WebGL2 canvas that draws a session's chart as it is edited, at the buyer
// renderer's quality and levels of detail, with the session's overlay
// (selection, handles, previews, guides, outlines, markers) over it.
//
// The host owns the view and every gesture: it gives the canvas a `View`
// (plain data), forwards input to `session.input` itself, and calls
// `draw()` from its animation frame when something changed. The canvas
// draws exactly that view and keeps no camera of its own.
import { createRasteriser, type Rasteriser, type TextRequest } from "@cavea/renderer/text";
import { loadGlyphAtlas, type GlyphAtlasSource } from "@cavea/renderer/atlas";

import type { CanvasHandle } from "./bindings.ts";
import type { Rect } from "./generated/calls.ts";
import { DesignerError, internals, type Module, type Session } from "./session.ts";

/** A chart point or vector, in chart units; or a canvas point in CSS pixels. */
export type Point = [number, number];

/**
 * What the canvas shows: the chart point at its centre, CSS pixels per chart
 * unit, its size in CSS pixels and the device pixel ratio.
 */
export interface View {
  centre: Point;
  zoom: number;
  width: number;
  height: number;
  dpr: number;
}

export type Theme = "light" | "dark";

export interface CanvasOptions {
  /** Default `"light"`; the backgrounds are `#FAFAF9` and `#1C1917`. */
  theme?: Theme;
  /**
   * The seat labels' glyph atlas (`packages/renderer/assets/glyphs.png`):
   * its URL, or `false` for none (no seat numbers). Default: the file in
   * `@cavea/renderer`.
   */
  glyphs?: GlyphAtlasSource | false;
  /**
   * Called when the canvas needs a frame the host could not know about: the
   * glyph atlas arrived, or a lost WebGL context came back. The host then
   * calls `draw()` from its next animation frame.
   */
  onFrame?: () => void;
  /** Errors the renderer reports (a GPU failure); the canvas keeps going. */
  onError?: (error: { code: string; message: string }) => void;
}

/** What the last frame drew, for the host's metrics. */
export interface CanvasStats {
  drawCalls: number;
  seatsDrawn: number;
  /** `overview`, `crossFade`, `seats`, `detail`, or `null` before a frame. */
  level: "overview" | "crossFade" | "seats" | "detail" | null;
  ratio: number;
  /** Pieces of the chart compiled, and merges, since the canvas was made. */
  compiled: number;
  merges: number;
}

const LEVELS = ["overview", "crossFade", "seats", "detail"] as const;

/** What the module reports after a draw. */
type Reported = TextRequest | { type: "error"; code: string; message: string };

/** The chart point under a canvas point, for `view`. */
export function toChart(view: View, [x, y]: Point): Point {
  return [
    view.centre[0] + (x - view.width / 2) / view.zoom,
    view.centre[1] + (y - view.height / 2) / view.zoom,
  ];
}

/** A chart point on the canvas, in CSS pixels from its top left, for `view`. */
export function toScreen(view: View, [x, y]: Point): Point {
  return [
    (x - view.centre[0]) * view.zoom + view.width / 2,
    (y - view.centre[1]) * view.zoom + view.height / 2,
  ];
}

/**
 * `view` zoomed by `factor` about a canvas point, which keeps the chart
 * point under it (the wheel), within `limits` when given.
 */
export function zoomAbout(
  view: View,
  factor: number,
  at: Point,
  limits: [number, number] | null = null,
): View {
  let zoom = view.zoom * factor;
  if (limits !== null) {
    zoom = Math.min(Math.max(zoom, limits[0]), limits[1]);
  }
  const before = toChart(view, at);
  const after = toChart({ ...view, zoom }, at);
  return {
    ...view,
    zoom,
    centre: [view.centre[0] + before[0] - after[0], view.centre[1] + before[1] - after[1]],
  };
}

/** `view` moved by `delta` CSS pixels (a pan: the chart follows the pointer). */
export function pan(view: View, [dx, dy]: Point): View {
  return { ...view, centre: [view.centre[0] - dx / view.zoom, view.centre[1] - dy / view.zoom] };
}

/**
 * The view of `view`'s size that shows `box` (`session.fit()` for zoom to
 * fit, `selectionView().bounds` for zoom to selection) with `padding` CSS
 * pixels around it, within `limits` when given.
 */
export function fitView(
  view: View,
  box: Rect,
  padding = 24,
  limits: [number, number] | null = null,
): View {
  const room = (px: number) => Math.max(px - 2 * padding, 1);
  const w = Math.max(box.max[0] - box.min[0], 1e-9);
  const h = Math.max(box.max[1] - box.min[1], 1e-9);
  let zoom = Math.min(room(view.width) / w, room(view.height) / h);
  if (limits !== null) {
    zoom = Math.min(Math.max(zoom, limits[0]), limits[1]);
  }
  return {
    ...view,
    zoom,
    centre: [(box.min[0] + box.max[0]) / 2, (box.min[1] + box.max[1]) / 2],
  };
}

/**
 * Attaches a canvas to `element` (WebGL2) for `session`. Throws a
 * `DesignerError` when WebGL2 is not available. Give it a view before the
 * first `draw()`.
 */
export function createCanvas(
  session: Session,
  element: HTMLCanvasElement,
  options: CanvasOptions = {},
): Canvas {
  return new Canvas(session, element, options);
}

export class Canvas {
  readonly #session: Session;
  readonly #module: Module;
  readonly #element: HTMLCanvasElement;
  readonly #options: CanvasOptions;
  #handle: CanvasHandle | null;
  #rasteriser: Rasteriser | null = createRasteriser();
  #view: View | null = null;
  #theme: Theme = "light";
  readonly #lost = (event: Event): void => {
    // Lets the browser restore the context (04 §20.2).
    event.preventDefault();
    this.#call((h) => h.contextLost());
  };
  readonly #restored = (): void => {
    this.#call((h) => h.contextRestored());
    this.#options.onFrame?.();
  };

  /** Use `createCanvas`. */
  constructor(session: Session, element: HTMLCanvasElement, options: CanvasOptions) {
    const { module } = internals(session);
    if (module.broken !== null) {
      throw new DesignerError({ kind: "panic", message: module.broken });
    }
    this.#session = session;
    this.#module = module;
    this.#element = element;
    this.#options = options;
    try {
      this.#handle = new module.bindings.Canvas(element);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new DesignerError({ kind: "arguments", message: `canvas: ${message}` });
    }
    element.addEventListener("webglcontextlost", this.#lost);
    element.addEventListener("webglcontextrestored", this.#restored);
    this.setTheme(options.theme ?? "light");
    const glyphs = options.glyphs ?? undefined;
    if (glyphs !== false) {
      loadGlyphAtlas(glyphs).then(
        (rgba) => {
          if (this.#handle !== null) {
            this.#call((h) => h.setGlyphAtlas(rgba));
            this.#options.onFrame?.();
          }
        },
        (error: unknown) => {
          const message = error instanceof Error ? error.message : String(error);
          this.#options.onError?.({ code: "glyphAtlas", message });
        },
      );
    }
  }

  /** Runs a call on the module, turning a trap into a `panic`. */
  #call<T>(run: (handle: CanvasHandle) => T): T {
    const handle = this.#handle;
    if (this.#module.broken !== null) {
      throw new DesignerError({ kind: "panic", message: this.#module.broken });
    }
    if (handle === null) {
      throw new DesignerError({ kind: "disposed" });
    }
    try {
      return run(handle);
    } catch (error) {
      this.#module.broken = error instanceof Error ? error.message : String(error);
      this.#handle = null;
      throw new DesignerError({ kind: "panic", message: this.#module.broken });
    }
  }

  /**
   * Sets what the canvas shows. The host calls it whenever its view
   * changes (pan, zoom) and when the canvas element's CSS size or the
   * device pixel ratio changes (a resize; the backing store follows at the
   * next draw). Throws an `arguments` failure, changing nothing, for a
   * number that is not finite, or a zoom or ratio that is not positive.
   */
  setView(view: View): void {
    const ok = this.#call((h) =>
      h.setView(view.centre[0], view.centre[1], view.zoom, view.width, view.height, view.dpr),
    );
    if (!ok) {
      throw new DesignerError({ kind: "arguments", message: "view is not finite" });
    }
    this.#view = { ...view, centre: [view.centre[0], view.centre[1]] };
  }

  get view(): View | null {
    return this.#view;
  }

  setTheme(theme: Theme): void {
    this.#theme = theme;
    this.#call((h) => h.setTheme(theme === "dark"));
  }

  get theme(): Theme {
    return this.#theme;
  }

  /**
   * Brings the picture up to date with the session and draws it if
   * anything changed (the document, the overlay, the view, the theme, text
   * arriving); a call that finds nothing changed draws nothing. Call it
   * from the animation frame after the session changed (`changed()` names
   * `overlay`, or anything was applied) or the view did. Returns whether
   * the canvas wants another frame (labels and text still arriving, moved
   * nodes still being baked): then call it again next frame.
   */
  draw(timeMs: number = performance.now()): boolean {
    const { handle: designer } = internals(this.#session);
    if (designer === null) {
      throw new DesignerError({ kind: "disposed" });
    }
    let again = this.#call((h) => h.draw(designer, timeMs));
    const events = JSON.parse(this.#call((h) => h.takeEvents())) as Reported[];
    for (const event of events) {
      if (event.type === "textRequest") {
        const raster = this.#rasteriser?.draw(event) ?? [0, 0, 0, new Uint8Array(0)];
        again = this.#call((h) => h.setText(event.id, ...raster)) || again;
      } else {
        this.#options.onError?.({ code: event.code, message: event.message });
      }
    }
    return again;
  }

  /** The chart point under a canvas point, for the current view. */
  toChart(point: Point): Point {
    return toChart(this.#requireView(), point);
  }

  /** A chart point on the canvas, for the current view. */
  toScreen(point: Point): Point {
    return toScreen(this.#requireView(), point);
  }

  /**
   * The zoom range the renderer is tuned for at the current size, as the
   * buyer's (04 §13): from 0.8 of the zoom that fits the floor shown to
   * where a default seat is 64 CSS px. `null` before the first frame with
   * a chart.
   */
  zoomLimits(): [number, number] | null {
    const [min, max] = this.#call((h) => h.zoomLimits());
    return min !== undefined && max !== undefined ? [min, max] : null;
  }

  /** The view of the current size that shows `box`, with `padding` px around it. */
  fit(box: Rect, padding = 24): View {
    return fitView(this.#requireView(), box, padding, this.zoomLimits());
  }

  stats(): CanvasStats {
    const s = this.#call((h) => h.frameStats());
    const level = LEVELS[s[2] ?? -1] ?? null;
    return {
      drawCalls: s[0] ?? 0,
      seatsDrawn: s[1] ?? 0,
      level,
      ratio: s[3] ?? 1,
      compiled: s[4] ?? 0,
      merges: s[5] ?? 0,
    };
  }

  /** Releases the WebGL resources and listeners. Idempotent. */
  dispose(): void {
    this.#element.removeEventListener("webglcontextlost", this.#lost);
    this.#element.removeEventListener("webglcontextrestored", this.#restored);
    if (this.#module.broken === null) {
      this.#handle?.free();
    }
    this.#handle = null;
    this.#rasteriser?.release();
    this.#rasteriser = null;
  }

  get disposed(): boolean {
    return this.#handle === null;
  }

  #requireView(): View {
    if (this.#view === null) {
      throw new DesignerError({ kind: "arguments", message: "no view: call setView first" });
    }
    return this.#view;
  }
}
