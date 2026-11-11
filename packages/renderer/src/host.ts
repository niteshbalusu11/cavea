// The web host for a `View` (docs/04-renderer.md §2, §14, §18, §21): the clock
// and frame scheduler, input forwarding and resize observation. The engine reads
// no clock: every command carries `event.timeStamp` or the
// `requestAnimationFrame` time, both on the `performance.now()` clock.
import { defaultMaxDpr, type View } from "../wasm/cavea_web.js";

export type GestureHandling = "greedy" | "cooperative" | "none";

export interface AttachOptions {
  /** Default `"greedy"`. */
  gestureHandling?: GestureHandling;
  /** Highest device pixel ratio to render at. Default 2 on touch devices, 3 otherwise. */
  maxDpr?: number;
  /** Default: follow `prefers-reduced-motion`. */
  reducedMotion?: boolean;
  /** Called after every frame the host runs, with its time. */
  onFrame?: (time: number) => void;
}

export interface Attached {
  setGestureHandling(mode: GestureHandling): void;
  /** Schedules a frame if the view wants one; call after using the view directly. */
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
  view: View,
  canvas: HTMLCanvasElement,
  options: AttachOptions = {},
): Attached {
  const maxDpr = options.maxDpr ?? defaultMaxDpr(matchMedia("(pointer: coarse)").matches);
  /** Pointers pressed on the canvas, by id, with their type code. */
  const pressed = new Map<number, number>();
  const cleanup: Array<() => void> = [];
  let handle = 0;
  let detached = false;

  function update(): void {
    if (view.needFrame && handle === 0 && !document.hidden && !detached) {
      handle = requestAnimationFrame(frame);
    }
  }

  function frame(time: number): void {
    handle = 0;
    view.frame(time);
    options.onFrame?.(time);
    update();
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
    view.pointer(
      kind,
      event.pointerId,
      pointerType(event.pointerType),
      event.clientX - rect.left,
      event.clientY - rect.top,
      event.timeStamp,
    );
    update();
  }

  function cancelAll(time: number): void {
    for (const [id, type] of pressed) {
      view.pointer(CANCEL, id, type, 0, 0, time);
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
      const taken = view.wheel(
        event.deltaX * scale,
        event.deltaY * scale,
        event.clientX - rect.left,
        event.clientY - rect.top,
        event.ctrlKey,
        event.timeStamp,
      );
      if (taken) {
        event.preventDefault();
      }
      update();
    },
    false,
  );

  const onVisibility = (): void => {
    if (document.hidden) {
      cancelAnimationFrame(handle);
      handle = 0;
      // Input stops while hidden; releases may never arrive.
      cancelAll(performance.now());
    } else {
      update();
    }
  };
  document.addEventListener("visibilitychange", onVisibility);
  cleanup.push(() => document.removeEventListener("visibilitychange", onVisibility));

  const resize = (): void => {
    view.resize(canvas.clientWidth, canvas.clientHeight, devicePixelRatio, maxDpr);
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
    },
  };
}
