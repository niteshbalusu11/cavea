// The text rasteriser (docs/04-renderer.md §12): draws the strings the engine
// asks for with `textRequest` events on a small 2D canvas, so the browser
// does the shaping, bidirectional ordering and font fallback, and hands back
// one byte of coverage per pixel with `View.setText`. It runs after a frame,
// never inside one, and works the same on an `OffscreenCanvas` in a worker.
// The font is a family the page already has (system fonts by default):
// nothing is fetched (12 §3 T20). Text is only ever drawn, never parsed.

/** A request as the engine emits it. */
export interface TextRequest {
  type: "textRequest";
  id: number;
  text: string;
  /** Device pixels per em. */
  px: number;
  weight: number;
  /** A CSS font family list. */
  font: string;
}

/** The engine's limits on a raster (`text::atlas` in cavea-render). */
const MAX_WIDTH = 1024;
const MAX_HEIGHT = 512;
/** Transparent pixels around the text. */
const PAD = 2;

type Context = OffscreenCanvasRenderingContext2D | CanvasRenderingContext2D;

/** The ink's width from the pen, or the advance if wider. */
const width = (metrics: TextMetrics): number =>
  Math.ceil(
    Math.max(metrics.actualBoundingBoxLeft, 0) +
      Math.max(metrics.width, metrics.actualBoundingBoxRight),
  );

export interface Rasteriser {
  /** Draws `request`: width, height, the em drawn at and the coverage bytes; empty if it cannot. */
  draw(request: TextRequest): [number, number, number, Uint8Array];
  /** Releases the canvas. */
  release(): void;
}

export function createRasteriser(): Rasteriser {
  let context: Context | null = null;
  const canvas =
    typeof OffscreenCanvas === "undefined"
      ? document.createElement("canvas")
      : new OffscreenCanvas(1, 1);
  const empty: [number, number, number, Uint8Array] = [0, 0, 0, new Uint8Array(0)];
  return {
    draw({ text, px, weight, font }) {
      context ??= canvas.getContext("2d", { willReadFrequently: true }) as Context | null;
      if (context === null) {
        return empty;
      }
      const c = context;
      const measure = (size: number): TextMetrics => {
        c.font = `${weight} ${size}px ${font}`;
        return c.measureText(text);
      };
      let size = px;
      let m = measure(size);
      if (width(m) + 2 * PAD > MAX_WIDTH) {
        size = Math.max(1, Math.floor((size * (MAX_WIDTH - 2 * PAD)) / width(m)));
        m = measure(size);
      }
      const ascent = Math.ceil(Math.max(m.fontBoundingBoxAscent, m.actualBoundingBoxAscent));
      const descent = Math.ceil(Math.max(m.fontBoundingBoxDescent, m.actualBoundingBoxDescent));
      const w = Math.min(MAX_WIDTH, width(m) + 2 * PAD);
      const h = Math.min(MAX_HEIGHT, ascent + descent + 2 * PAD);
      if (!(w > 2 * PAD && h > 2 * PAD)) {
        return empty;
      }
      // Resizing clears the canvas and its state.
      canvas.width = w;
      canvas.height = h;
      c.font = `${weight} ${size}px ${font}`;
      c.fillStyle = "#fff";
      c.fillText(text, PAD + Math.max(m.actualBoundingBoxLeft, 0), PAD + ascent);
      const rgba = c.getImageData(0, 0, w, h).data;
      const coverage = new Uint8Array(w * h);
      for (let i = 0; i < coverage.length; i++) {
        coverage[i] = rgba[i * 4 + 3] ?? 0;
      }
      return [w, h, size, coverage];
    },
    release() {
      canvas.width = 0;
      canvas.height = 0;
      context = null;
    },
  };
}
