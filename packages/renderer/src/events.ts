// Engine events as the host receives them: one batch per frame or command,
// never one call per seat (docs/04-renderer.md §3, §16). The public SDK
// (docs/05-web-sdk.md §5) builds its callbacks on these.
import type { View } from "../wasm/cavea_web.js";

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

export type RendererEvent =
  /** Hold these labels, then answer with `holdOk` or `holdFailed`. */
  | { type: "holdRequest"; request: number; labels: string[] }
  /** Release these labels; there is no answer. */
  | { type: "releaseRequest"; request: number; labels: string[] }
  | { type: "selectionChanged"; selected: ObjectInfo[] }
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
  | {
      type: "cameraChanged";
      centre: [number, number];
      zoom: number;
      lod: "overview" | "crossFade" | "seats" | "detail";
    };

/** The events `view` emitted since the last call, oldest first. */
export function takeEvents(view: View): RendererEvent[] {
  const json = view.takeEvents();
  return json === undefined ? [] : (JSON.parse(json) as RendererEvent[]);
}
