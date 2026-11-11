// Engine events as the host receives them: one batch per frame or command,
// never one call per seat (docs/04-renderer.md §3, §16). The public SDK
// (docs/05-web-sdk.md §5) builds its callbacks on these.
import type { ChartInfo, EngineEvent, ObjectInfo } from "./generated/engine.ts";
import type { EngineView } from "./view.ts";

export type { ChartInfo, ObjectInfo };

/** What the engine reports, and what the host adds. */
export type RendererEvent =
  | EngineEvent
  /**
   * A host-side failure the map carries on without: `glyphAtlas` when the
   * atlas could not be fetched, decoded or used, so seats have no labels;
   * `engine` when a frame or input call threw; `image` when a background
   * image could not be fetched, decoded or used.
   */
  | { type: "error"; code: "glyphAtlas" | "engine" | "image"; message: string };

/** The events `view` emitted since the last call, oldest first. */
export function takeEvents(view: EngineView): RendererEvent[] {
  const json = view.takeEvents();
  return json === undefined ? [] : (JSON.parse(json) as RendererEvent[]);
}
