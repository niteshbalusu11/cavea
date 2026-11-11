// The canvas region (07 §3, §4): a WebGL2 `Canvas` from
// `@cavea/designer-core` drawing the session at the editor's view. The
// camera lives in the editor alone; this region measures itself and the
// pixel ratio into it, draws from the frame loop (`paint.ts`: only when
// something changed), turns pointer events into the session's `Pointer`s
// at that same view, and pans and zooms the camera: the wheel pans,
// Ctrl or Cmd with the wheel (a trackpad's pinch) and a two-finger pinch
// zoom about the pointer, and the session's `pan` (the hand tool, Space,
// the middle button) moves it.
import {
  createCanvas,
  DesignerError,
  toChart,
  type Button,
  type Canvas,
  type Cursor,
  type Input,
  type Pointer,
  type PointerKind,
  type Theme,
} from "@cavea/designer-core";
import { createEffect, createSignal, on, onCleanup, onMount, Show } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { notify } from "../ui/overlays.tsx";
import type { Editor } from "./editor.ts";
import { modifiers } from "./keys.ts";
import { Painter } from "./paint.ts";
import { pinchStep, wheelStep, type Pinch } from "./wheel.ts";

const BUTTONS: Record<number, Button> = { 0: "primary", 1: "middle", 2: "secondary" };

const themeOf = (): Theme =>
  document.documentElement.dataset["theme"] === "dark" ? "dark" : "light";

export function CanvasHost(props: { editor: Editor }) {
  const editor = props.editor;
  let region!: HTMLDivElement;
  let element!: HTMLCanvasElement;
  /** Why there is no canvas (no WebGL2, a failed draw), shown in its place. */
  const [failed, setFailed] = createSignal<string | null>(null);
  let canvas: Canvas | null = null;
  let painter: Painter | null = null;
  /** The pointer whose press the session is following, if any. */
  let held: number | null = null;
  /** Touch points down, by pointer id, for the pinch. */
  const touches = new Map<number, [number, number]>();
  let pinch: Pinch | null = null;
  /** The pointer over the canvas (CSS px), for the hovered seat's sample. */
  let over: [number, number] | null = null;
  /** The numbering panel's sample of the hovered seat (07 §6): its full label. */
  const [sample, setSample] = createSignal<{ at: [number, number]; label: string } | null>(null);
  const resample = () => {
    if (over === null || editor.views.numbering === null) return setSample(null);
    const view = editor.view();
    const at = over;
    const seat = editor.run((s) => s.seatUnder(toChart(view, at), view.zoom));
    const label = seat ? editor.run((s) => s.sample(seat.node, seat.oid)) : null;
    setSample(typeof label === "string" ? { at, label } : null);
  };
  // The panel closed: no sample.
  createEffect(() => {
    if (editor.views.numbering === null) setSample(null);
  });

  const local = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = region.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };
  const pointer = (e: PointerEvent): Pointer => {
    const screen = local(e);
    const view = editor.view();
    return {
      world: toChart(view, screen),
      screen,
      zoom: view.zoom,
      button: BUTTONS[e.button] ?? "primary",
      kind: (["mouse", "touch", "pen"].includes(e.pointerType)
        ? e.pointerType
        : "mouse") as PointerKind,
      mods: modifiers(e),
      time: e.timeStamp,
    };
  };
  const send = (input: Input) => editor.input(input);
  /** Ends the gesture the session is following, as a cancel. */
  const cancel = () => {
    if (held === null) return;
    const id = held;
    held = null;
    if (element.hasPointerCapture(id)) element.releasePointerCapture(id);
    send({ kind: "pointerCancel" });
  };

  const cursor = (to: Cursor) => {
    element.style.cursor = to;
  };
  /** Stops drawing and says why; the panels stay usable. */
  const fail = (message: string) => {
    editor.loop.setPainter(null);
    canvas?.dispose();
    canvas = null;
    painter = null;
    setFailed(message);
  };

  onMount(() => {
    const measure = () => {
      editor.setSize([region.clientWidth, region.clientHeight]);
      editor.setDpr(window.devicePixelRatio || 1);
    };
    const resize = new ResizeObserver(measure);
    resize.observe(region);
    measure();
    // A pixel-ratio change (a window moved to another screen, the
    // browser's zoom) does not always resize the region.
    let ratio: MediaQueryList | null = null;
    const watchRatio = () => {
      ratio?.removeEventListener("change", onRatio);
      ratio = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
      ratio.addEventListener("change", onRatio);
    };
    const onRatio = () => {
      measure();
      watchRatio();
    };
    watchRatio();
    // The console's theme is `data-theme` on the root element.
    const theme = new MutationObserver(() => {
      if (canvas === null) return;
      const t = themeOf();
      if (t !== canvas.theme) {
        canvas.setTheme(t);
        painter?.invalidate();
      }
    });
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    window.addEventListener("blur", cancel);
    region.addEventListener("wheel", onWheel, { passive: false });
    onCleanup(() => {
      resize.disconnect();
      ratio?.removeEventListener("change", onRatio);
      theme.disconnect();
      window.removeEventListener("blur", cancel);
      region.removeEventListener("wheel", onWheel);
    });

    let reported = false;
    try {
      canvas = createCanvas(editor.session, element, {
        theme: themeOf(),
        onFrame: () => painter?.invalidate(),
        onError: () => {
          // A GPU error the canvas survives: said once, not every frame.
          if (!reported) notify(msg("canvas.gpuError"), "error");
          reported = true;
        },
      });
    } catch (error) {
      const panicked = error instanceof DesignerError && error.failure.kind === "panic";
      return setFailed(
        panicked ? msg("canvas.failed", { message: error.message }) : msg("canvas.noWebgl"),
      );
    }
    const c = canvas;
    painter = new Painter(
      {
        draw(time) {
          const again = c.draw(time);
          editor.setLimits(c.zoomLimits());
          return again;
        },
      },
      (names) => {
        if (names.has("cursor")) cursor(editor.session.cursor());
        // A new preview relabels the seat under a still pointer.
        if (names.has("numbering")) resample();
      },
    );
    cursor(editor.session.cursor());
    const p = painter;
    editor.loop.setPainter({
      changed: (names) => p.changed(names),
      frame(time) {
        try {
          p.frame(time);
        } catch (error) {
          // A trap stops the loop and shows the reload dialog; anything
          // else stops only the canvas.
          if (error instanceof DesignerError && error.failure.kind === "panic") throw error;
          fail(msg("canvas.failed", { message: String(error) }));
        }
      },
    });
    // Before the session goes, and when this region does.
    const dispose = () => {
      editor.loop.setPainter(null);
      c.dispose();
    };
    editor.attach(dispose);
    onCleanup(dispose);
    if (import.meta.env.DEV) Object.assign(window, { caveaCanvas: { canvas: c, painter: p } });
  });

  // The view, to the canvas: every camera, size or ratio change.
  createEffect(() => {
    const view = editor.view();
    if (canvas === null || view.width <= 0 || view.height <= 0) return;
    canvas.setView(view);
    painter?.invalidate();
  });
  // Zoom to fit on the first measure.
  createEffect(
    on(
      () => editor.size()[0] > 0,
      (measured) => {
        if (measured) editor.frame(editor.run((s) => s.fit()) ?? null);
      },
    ),
  );

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const step = wheelStep(e, region.clientHeight);
    if (step.zoom !== null) editor.zoomBy(step.zoom, local(e));
    else editor.pan(step.pan);
  }

  const touchDown = (e: PointerEvent): boolean => {
    if (e.pointerType !== "touch") return false;
    touches.set(e.pointerId, local(e));
    if (touches.size !== 2) return false;
    // A second finger: the gesture becomes a pinch.
    cancel();
    pinch = { points: [...touches.values()] as Pinch["points"] };
    return true;
  };
  const touchMove = (e: PointerEvent): boolean => {
    if (!touches.has(e.pointerId)) return false;
    touches.set(e.pointerId, local(e));
    if (pinch === null || touches.size !== 2) return pinch !== null;
    const next = [...touches.values()] as Pinch["points"];
    const step = pinchStep(pinch, next);
    pinch = { points: next };
    editor.pan(step.pan);
    editor.zoomBy(step.factor, step.at);
    return true;
  };
  const touchUp = (e: PointerEvent): boolean => {
    touches.delete(e.pointerId);
    if (pinch === null) return false;
    // The pinch lasts until every finger is up.
    if (touches.size === 0) pinch = null;
    return true;
  };

  return (
    <div
      ref={(e) => (region = e)}
      class="relative min-w-0 flex-1 touch-none overflow-hidden bg-canvas select-none"
    >
      <canvas
        ref={(e) => (element = e)}
        data-canvas
        tabIndex={0}
        role="application"
        aria-label={msg("canvas.label", { name: editor.views.title })}
        aria-roledescription={msg("canvas.role")}
        class="absolute inset-0 block size-full outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus"
        classList={{ hidden: failed() !== null }}
        onPointerDown={(e) => {
          if (e.button === 1) e.preventDefault();
          element.focus({ preventScroll: true });
          if (touchDown(e) || pinch !== null || held !== null) return;
          element.setPointerCapture(e.pointerId);
          held = e.pointerId;
          send({ kind: "pointerDown", data: pointer(e) });
        }}
        onPointerMove={(e) => {
          if (touchMove(e)) return;
          if (held !== null && e.pointerId !== held) return;
          const p = pointer(e);
          editor.setPointer(p.world);
          send({ kind: "pointerMove", data: p });
          over = p.screen;
          if (editor.views.numbering !== null) resample();
        }}
        onPointerUp={(e) => {
          if (touchUp(e) || e.pointerId !== held) return;
          held = null;
          send({ kind: "pointerUp", data: pointer(e) });
        }}
        onPointerCancel={(e) => {
          touchUp(e);
          if (e.pointerId === held) cancel();
        }}
        // Capture lost while the press is held (the element hidden, another
        // capture): the gesture ends as a cancel, not half way.
        onLostPointerCapture={(e) => {
          if (e.pointerId === held) cancel();
        }}
        onPointerLeave={() => {
          editor.setPointer(null);
          over = null;
          setSample(null);
        }}
        onContextMenu={(e) => e.preventDefault()}
        onMouseDown={(e) => {
          // No autoscroll on a middle press: it pans.
          if (e.button === 1) e.preventDefault();
        }}
      />
      <Show when={sample()}>
        {(s) => (
          <div
            aria-hidden="true"
            class="pointer-events-none absolute rounded-sm border border-line bg-raised px-1.5 py-0.5 font-mono text-xs text-fg shadow-menu"
            style={{ left: `${s().at[0] + 14}px`, top: `${s().at[1] + 14}px` }}
          >
            {s().label}
          </div>
        )}
      </Show>
      <Show when={failed()}>
        {(message) => (
          <div
            role="alert"
            class="absolute inset-0 flex flex-col items-center justify-center gap-1 p-6 text-center"
          >
            <p class="text-lg font-medium text-fg">{msg("canvas.unavailable")}</p>
            <p class="max-w-md text-base text-muted">{message()}</p>
            <p class="mt-2 max-w-md text-sm text-faint">{msg("canvas.panelsStill")}</p>
          </div>
        )}
      </Show>
    </div>
  );
}
