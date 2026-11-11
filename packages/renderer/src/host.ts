// The web host for a `View` (docs/04-renderer.md §2, §14, §18, §21): the clock
// and frame scheduler, input forwarding and resize observation. The engine reads
// no clock: every command carries `event.timeStamp` or the
// `requestAnimationFrame` time, both on the `performance.now()` clock.
import { defaultMaxDpr } from "../wasm/cavea_web.js";
import { takeEvents, type RendererEvent } from "./events.ts";
import { createRasteriser } from "./text.ts";
import type { EngineView } from "./view.ts";

export type GestureHandling = "greedy" | "cooperative" | "none";

export interface AttachOptions {
  /** Default `"greedy"`. */
  gestureHandling?: GestureHandling;
  /** Highest device pixel ratio to render at. Default 2 on touch devices, 3 otherwise. */
  maxDpr?: number;
  /** Default: follow `prefers-reduced-motion`. */
  reducedMotion?: boolean;
  /**
   * The glyph atlas (`Renderer.glyphAtlas`), given to the view when it
   * arrives; seat labels appear then. A rejection is reported as an `error`
   * event and the map carries on without labels.
   */
  glyphAtlas?: Promise<Uint8Array>;
  /**
   * Called after every frame the host runs, with its time and, with
   * `timeFrames`, the milliseconds the frame's work took (the view's frame,
   * text drawing and event delivery; GPU time is not counted), else 0.
   */
  onFrame?: (time: number, ms: number) => void;
  /** Time each frame for `onFrame`; off, the frame loop reads no clock. */
  timeFrames?: boolean;
  /** The WebGL context was lost (`false`) or restored (`true`), 04 §20.2. */
  onContext?: (restored: boolean) => void;
  /** Called when the page is hidden (`false`) or shown again (`true`). */
  onVisibility?: (visible: boolean) => void;
  /**
   * Called with the engine's events after each frame, and after input or an
   * `update()` that produced any, outside the frame.
   */
  onEvents?: (events: RendererEvent[]) => void;
}

export interface Attached {
  setGestureHandling(mode: GestureHandling): void;
  /**
   * Delivers pending events and schedules a frame if the view wants one; call
   * after using the view directly.
   */
  update(): void;
  /** Removes every listener and stops the frame loop. */
  detach(): void;
}

const DOWN = 0;
const MOVE = 1;
const UP = 2;
const CANCEL = 3;
const MODES: Record<GestureHandling, number> = { greedy: 0, cooperative: 1, none: 2 };
/** Pixels per wheel line: at least the engine's mouse-notch threshold, as only mice scroll by lines. */
const LINE_PX = 50;

function pointerType(type: string): number {
  return type === "mouse" ? 0 : type === "touch" ? 1 : 2;
}

/**
 * Drives `view` from `canvas`: forwards pointer and wheel input, observes size
 * and device-pixel-ratio changes, and runs `requestAnimationFrame` only while
 * the engine asks for frames and the page is visible.
 */
export function attach(
  view: EngineView,
  canvas: HTMLCanvasElement,
  options: AttachOptions = {},
): Attached {
  const maxDpr = options.maxDpr ?? defaultMaxDpr(matchMedia("(pointer: coarse)").matches);
  /** Pointers pressed on the canvas, by id, with their type code. */
  const pressed = new Map<number, number>();
  const cleanup: Array<() => void> = [];
  let handle = 0;
  let detached = false;
  /** While the context is lost no frame is run. */
  let lost = false;
  const rasteriser = createRasteriser();

  /** Runs a view call; a throw becomes an `error` event, never the page's. */
  function guard(call: () => unknown): void {
    try {
      call();
    } catch (error) {
      options.onEvents?.([{ type: "error", code: "engine", message: String(error) }]);
    }
  }

  function update(): void {
    if (detached) {
      return;
    }
    // Taken even without a listener, so they do not pile up in the view.
    // Text the engine asks for is drawn here, between frames (04 §12).
    const events = takeEvents(view).filter((event) => {
      if (event.type !== "textRequest") {
        return true;
      }
      guard(() => view.setText(event.id, ...rasteriser.draw(event)));
      return false;
    });
    if (events.length > 0) {
      options.onEvents?.(events);
    }
    // A listener may have detached the host and freed the view.
    if (!detached && !lost && view.needFrame && handle === 0 && !document.hidden) {
      handle = requestAnimationFrame(frame);
    }
  }

  function frame(time: number): void {
    handle = 0;
    const start = options.timeFrames === true ? performance.now() : 0;
    guard(() => view.frame(time));
    update();
    options.onFrame?.(time, start === 0 ? 0 : performance.now() - start);
  }

  function listen<K extends keyof HTMLElementEventMap>(
    type: K,
    listener: (event: HTMLElementEventMap[K]) => void,
    passive = true,
  ): void {
    canvas.addEventListener(type, listener, { passive });
    cleanup.push(() => canvas.removeEventListener(type, listener));
  }

  function send(event: PointerEvent, kind: number): void {
    const rect = canvas.getBoundingClientRect();
    guard(() =>
      view.pointer(
        kind,
        event.pointerId,
        pointerType(event.pointerType),
        event.clientX - rect.left,
        event.clientY - rect.top,
        event.timeStamp,
      ),
    );
    update();
  }

  function cancelAll(time: number): void {
    for (const [id, type] of pressed) {
      guard(() => view.pointer(CANCEL, id, type, 0, 0, time));
    }
    pressed.clear();
    update();
  }

  listen("pointerdown", (event) => {
    if (event.pointerType === "mouse" && event.button !== 0) {
      return;
    }
    pressed.set(event.pointerId, pointerType(event.pointerType));
    // Capture keeps a drag that leaves the canvas coming to it.
    canvas.setPointerCapture(event.pointerId);
    send(event, DOWN);
  });
  listen("pointermove", (event) => send(event, MOVE));
  listen("pointerup", (event) => {
    if (pressed.delete(event.pointerId)) {
      send(event, UP);
    }
  });
  listen("pointercancel", (event) => {
    pressed.delete(event.pointerId);
    send(event, CANCEL);
  });
  // A capture lost without an up or cancel would otherwise leave the pointer down.
  listen("lostpointercapture", (event) => {
    if (pressed.delete(event.pointerId)) {
      send(event, CANCEL);
    }
  });
  // A mouse or pen leaving while not pressed ends hover.
  listen("pointerleave", (event) => {
    if (!pressed.has(event.pointerId) && event.pointerType !== "touch") {
      send(event, CANCEL);
    }
  });
  // Not passive: the map prevents scrolling only for wheels it takes.
  listen(
    "wheel",
    (event) => {
      const scale =
        event.deltaMode === WheelEvent.DOM_DELTA_LINE
          ? LINE_PX
          : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
            ? canvas.clientHeight
            : 1;
      const rect = canvas.getBoundingClientRect();
      let taken = false;
      guard(
        () =>
          (taken = view.wheel(
            event.deltaX * scale,
            event.deltaY * scale,
            event.clientX - rect.left,
            event.clientY - rect.top,
            event.ctrlKey,
            event.timeStamp,
          )),
      );
      if (taken) {
        event.preventDefault();
      }
      update();
    },
    false,
  );

  // The keyboard (11 §3): only while the canvas has the page's focus, and
  // only the keys the map takes are kept from the page.
  listen(
    "keydown",
    (event) => {
      if (event.isComposing) {
        return;
      }
      const mods =
        (event.shiftKey ? 1 : 0) |
        (event.ctrlKey ? 2 : 0) |
        (event.altKey ? 4 : 0) |
        (event.metaKey ? 8 : 0);
      let taken = false;
      guard(() => (taken = view.key(event.key, mods)));
      if (taken) {
        event.preventDefault();
      }
      update();
    },
    false,
  );
  const focused = (shown: boolean): void => {
    guard(() => view.setFocusShown(shown));
    update();
  };
  // Focus from the keyboard shows the remembered focus; a click's does not.
  listen("focus", () => {
    let visible = true;
    try {
      visible = canvas.matches(":focus-visible");
    } catch {
      // No `:focus-visible`: show it.
    }
    focused(visible);
  });
  listen("blur", () => focused(false));

  // Not passive: preventing the default is what lets the browser restore
  // the context. The view stops drawing; restored, it makes everything again
  // from the engine's copies and redraws. (The DOM types lack these events.)
  const onLost = (event: Event): void => {
    event.preventDefault();
    lost = true;
    cancelAnimationFrame(handle);
    handle = 0;
    guard(() => view.contextLost());
    options.onContext?.(false);
  };
  const onRestored = (): void => {
    lost = false;
    guard(() => view.contextRestored());
    options.onContext?.(true);
    update();
  };
  canvas.addEventListener("webglcontextlost", onLost);
  canvas.addEventListener("webglcontextrestored", onRestored);
  cleanup.push(() => {
    canvas.removeEventListener("webglcontextlost", onLost);
    canvas.removeEventListener("webglcontextrestored", onRestored);
  });

  const onVisibility = (): void => {
    if (document.hidden) {
      cancelAnimationFrame(handle);
      handle = 0;
      // Input stops while hidden; releases may never arrive.
      cancelAll(performance.now());
    } else {
      update();
    }
    options.onVisibility?.(!document.hidden);
  };
  document.addEventListener("visibilitychange", onVisibility);
  cleanup.push(() => document.removeEventListener("visibilitychange", onVisibility));

  const resize = (): void => {
    guard(() => view.resize(canvas.clientWidth, canvas.clientHeight, devicePixelRatio, maxDpr));
    update();
  };
  const observer = new ResizeObserver(resize);
  observer.observe(canvas);
  cleanup.push(() => observer.disconnect());

  // Moving between screens or zooming the browser changes the ratio, and not
  // always the CSS size; this query matches only the current ratio.
  let ratioQuery: MediaQueryList | undefined;
  const onRatio = (): void => {
    ratioQuery?.removeEventListener("change", onRatio);
    ratioQuery = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
    ratioQuery.addEventListener("change", onRatio);
    resize();
  };
  onRatio();
  cleanup.push(() => ratioQuery?.removeEventListener("change", onRatio));

  if (options.reducedMotion === undefined) {
    const motion = matchMedia("(prefers-reduced-motion: reduce)");
    const onMotion = (): void => view.setReducedMotion(motion.matches);
    onMotion();
    motion.addEventListener("change", onMotion);
    cleanup.push(() => motion.removeEventListener("change", onMotion));
  } else {
    view.setReducedMotion(options.reducedMotion);
  }

  const atlasFailed = (error: unknown): void =>
    options.onEvents?.([{ type: "error", code: "glyphAtlas", message: String(error) }]);
  options.glyphAtlas?.then((pixels) => {
    if (detached) {
      return;
    }
    try {
      view.setGlyphAtlas(pixels);
    } catch (error) {
      atlasFailed(error);
    }
    update();
  }, atlasFailed);

  const setGestureHandling = (mode: GestureHandling): void => view.setGestureHandling(MODES[mode]);
  setGestureHandling(options.gestureHandling ?? "greedy");

  return {
    setGestureHandling,
    update,
    detach() {
      detached = true;
      cancelAnimationFrame(handle);
      handle = 0;
      cancelAll(performance.now());
      for (const off of cleanup.splice(0)) {
        off();
      }
      rasteriser.release();
    },
  };
}
