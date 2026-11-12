// A version's thumbnail (07 §11 step 4, 09 §3.1, DSN-040): after a publish,
// the chart as a buyer would see it, fitted whole on the floor shown, in
// the light theme, drawn on a small canvas of its own (never the editor's,
// whose view, selection and frames are untouched), uploaded as an asset
// and attached to the new version. The canvas draws the buyer's view of
// the session (`Canvas.showBuyer`: no overlay, no reference image), from
// the pieces the editor's canvas has compiled. A failure anywhere leaves
// the version without a thumbnail; it never fails the publish.
import {
  createCanvas,
  fitView,
  type Canvas,
  type ImageSource,
  type Rect,
  type Session,
  type View,
} from "@cavea/designer-core";

/** The picture's size in pixels: a chart card's 4:3, a few hundred a side. */
export const THUMBNAIL = { width: 480, height: 360 } as const;
/** Pixels left around the chart. */
const PADDING = 16;
/** Drawn again until nothing has arrived (text, the glyph atlas, a
 * picture) for this long, or for this long at most. */
const QUIET_MS = 300;
const LIMIT_MS = 5_000;
/** Timers, not animation frames: they run in a hidden tab too. */
const FRAME_MS = 16;

const later = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** `box` grown by `share` of its size on each side: the buyer's map opens
 * on the seats with room around them (04 §13: out to 0.8 of the fit). */
function grown(box: Rect, share: number): Rect {
  const dx = (box.max[0] - box.min[0]) * share;
  const dy = (box.max[1] - box.min[1]) * share;
  return { min: [box.min[0] - dx, box.min[1] - dy], max: [box.max[0] + dx, box.max[1] + dy] };
}

/**
 * Draws the session as a buyer would see it on a canvas of its own and
 * encodes it (WebP where the browser writes it, else PNG; both are assets
 * the server takes, D-49). `null` when there is nothing to draw or no
 * WebGL2. `editor`: the editor's canvas, whose compiled pieces are reused.
 */
export async function renderThumbnail(
  session: Session,
  editor: Canvas | null,
  images: ImageSource | undefined,
  /** Whether the session still shows what is pictured: checked before
   * each draw, `"changed"` (nothing encoded) once it does not. */
  same: () => boolean = () => true,
): Promise<Blob | null | "changed"> {
  const element = document.createElement("canvas");
  // Kept after the frame that drew it, so it can be read at the end: this
  // canvas is thrown away, the editor's keeps the cheaper default.
  const gl = element.getContext("webgl2", {
    alpha: false,
    antialias: false,
    depth: false,
    stencil: false,
    preserveDrawingBuffer: true,
  });
  if (gl === null) return null;
  let arrived = false;
  const canvas = createCanvas(session, element, {
    theme: "light",
    onFrame: () => {
      arrived = true;
    },
    ...(images === undefined ? {} : { images }),
  });
  try {
    canvas.showBuyer(editor ?? undefined);
    // The first draw merges the scene, to learn its bounds; at a zoom that
    // shows it as a dot, so it costs the GPU next to nothing. The fitted
    // draw comes in a task of its own, so neither is a long one.
    const base: View = { centre: [0, 0], zoom: 1e-6, ...THUMBNAIL, dpr: 1 };
    canvas.setView(base);
    canvas.draw();
    const box = canvas.sceneBounds() ?? session.fit();
    if (box === null) return null;
    await later(FRAME_MS);
    if (!same()) return "changed";
    canvas.setView(fitView(base, grown(box, 0.1), PADDING));
    const start = performance.now();
    let quiet = start;
    for (;;) {
      if (!same()) return "changed";
      const busy = canvas.draw() || arrived;
      arrived = false;
      const now = performance.now();
      if (busy) quiet = now;
      if (now - quiet >= QUIET_MS || now - start >= LIMIT_MS) break;
      await later(FRAME_MS);
    }
    if (!same()) return "changed";
    return await new Promise<Blob | null>((resolve) => element.toBlob(resolve, "image/webp", 0.9));
  } finally {
    canvas.dispose();
    gl.getExtension("WEBGL_lose_context")?.loseContext();
  }
}

/** One request's outcome. */
export type Sent<T> = { ok: T } | { offline: true } | { failed: true };

export interface ThumbnailSteps<V> {
  render(): Promise<Blob | null>;
  /** `POST /v1/assets`: the asset's hash. */
  upload(blob: Blob): Promise<Sent<string>>;
  /** `PUT …/versions/{number}/thumbnail`: the version with it. */
  attach(hash: string): Promise<Sent<V>>;
  wait(ms: number): Promise<void>;
}

/** Tries after no answer: at once, then after 2 and 4 s. */
const WAITS = [2_000, 4_000];

/**
 * Renders, uploads and attaches a thumbnail; the version with it, or
 * `null` when any step failed (each tried again on no answer). Never
 * rejects. The same picture published again is the same asset: its hash
 * is its content's (D-49), so the upload stores nothing new.
 */
export async function attachThumbnail<V>(steps: ThumbnailSteps<V>): Promise<V | null> {
  let blob: Blob | null;
  try {
    blob = await steps.render();
  } catch {
    return null;
  }
  if (blob === null) return null;
  const image = blob;
  const retried = async <T>(send: () => Promise<Sent<T>>): Promise<T | null> => {
    for (let attempt = 0; ; attempt++) {
      const sent = await send().catch((): Sent<T> => ({ failed: true }));
      if ("ok" in sent) return sent.ok;
      const wait = WAITS[attempt];
      if ("failed" in sent || wait === undefined) return null;
      await steps.wait(wait);
    }
  };
  const hash = await retried(() => steps.upload(image));
  return hash === null ? null : retried(() => steps.attach(hash));
}
