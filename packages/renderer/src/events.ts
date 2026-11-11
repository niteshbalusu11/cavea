// Engine events as the host receives them: one batch per frame or command,
// never one call per seat (docs/04-renderer.md §3, §16). The public SDK
// (docs/05-web-sdk.md §5) builds its callbacks on these.
import type { ImageRequest } from "./images.ts";
import type { TextRequest } from "./text.ts";
import type { EngineView } from "./view.ts";

/** One object as the engine reports it. */
export interface ObjectInfo {
  /** Full label, for example `"Orchestra-C-12"`. */
  label: string;
  /** Category key. */
  category: string;
  /** Section label; empty outside a section. */
  section: string;
  row: string;
  seat: string;
  /** `SFLG` bits: 0 accessible, 1 companion, 2 restricted view, 3 aisle. */
  flags: number;
}

/** Chart metadata, from the engine's `Ready` (04 §3). */
export interface ChartInfo {
  name: string;
  floors: Array<{ name: string; level: number; seats: number }>;
  /** `floor`: index into `floors`. */
  sections: Array<{ label: string; floor: number; seats: number }>;
  /** `color`: RGBA bytes; `objects`: bookable objects in the category. */
  categories: Array<{ key: string; label: string; color: number[]; objects: number }>;
  objects: number;
  seats: number;
}

export type RendererEvent =
  /** A chart was loaded. */
  | ({ type: "ready" } & ChartInfo)
  /** Hold these labels, then answer with `holdOk` or `holdFailed`. */
  | { type: "holdRequest"; request: number; labels: string[] }
  /** Release these labels; there is no answer. */
  | { type: "releaseRequest"; request: number; labels: string[] }
  /** `expiresAt`: when the buyer's holds expire (Unix ms), if known. */
  | { type: "selectionChanged"; expiresAt: number | null; selected: ObjectInfo[] }
  /**
   * Rule `min`: the selection is not ready for checkout (`objects` is the
   * selection). Any other rule (`max`, `not_free`, a server code): `objects`
   * were refused and the selection is as it was.
   */
  | { type: "selectionInvalid"; rule: string; objects: string[] }
  /** The selection meets the minimum again. */
  | { type: "selectionValid" }
  /** `at`: the object's centre in CSS pixels on the canvas. */
  | { type: "objectHover"; object: ObjectInfo; at: [number, number] }
  | { type: "objectHover"; object: null; at: null }
  | { type: "objectFocus"; object: ObjectInfo; at: [number, number] }
  | { type: "objectFocus"; object: null; at: null }
  /** A selected object was taken; it is no longer selected. */
  | { type: "objectUnavailable"; label: string }
  /** The buyer's hold on these selected objects is gone (expired, or released elsewhere). */
  | { type: "holdLost"; labels: string[] }
  /** A delta did not follow the state at `seq`: fetch a new snapshot. */
  | { type: "resync"; seq: number | null }
  /**
   * A host-side failure the map carries on without: `glyphAtlas` when the
   * atlas could not be fetched, decoded or used, so seats have no labels;
   * `engine` when a frame or input call threw; `image` when a background
   * image could not be fetched, decoded or used.
   */
  | { type: "error"; code: "glyphAtlas" | "engine" | "image"; message: string }
  /** Draw this string and answer with `setText`; the host does, between frames. */
  | TextRequest
  /** Load this background image and answer with `setImage`; the host does. */
  | ImageRequest
  | {
      type: "cameraChanged";
      centre: [number, number];
      zoom: number;
      /** The lowest and highest zoom, for the current chart and size (04 §13). */
      range: [number, number];
      lod: "overview" | "crossFade" | "seats" | "detail";
    };

/** The events `view` emitted since the last call, oldest first. */
export function takeEvents(view: EngineView): RendererEvent[] {
  const json = view.takeEvents();
  return json === undefined ? [] : (JSON.parse(json) as RendererEvent[]);
}
