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
import { createEffect, createSignal, onCleanup, onMount, Show, untrack } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { notify } from "../ui/overlays.tsx";
import type { Editor } from "./editor.ts";
import { modifiers } from "./keys.ts";
import { Painter } from "./paint.ts";
import { guideNear, Rulers, type GuideDrag, type RulersHandle } from "./Rulers.tsx";
import { pinchStep, wheelStep, type Pinch } from "./wheel.ts";

const BUTTONS: Record<number, Button> = { 0: "primary", 1: "middle", 2: "secondary" };

const themeOf = (): Theme =>
  document.documentElement.dataset["theme"] === "dark" ? "dark" : "light";

export function CanvasHost(props: { editor: Editor; rulers: boolean }) {
  const editor = props.editor;
  let rulers: RulersHandle | null = null;
  /** A user guide being dragged on the canvas (07 §5.2). */
  let guideDrag: GuideDrag | null = null;
  /** The guide under a still pointer, for the cursor: its axis. */
  const guideAt = (at: [number, number]): GuideDrag | null => {
    if (editor.views.tool.tool !== "select") return null;
    const view = editor.view();
    const guides = editor.views.guides;
    const v = guideNear(view, guides, "vertical", at[0]);
    if (v !== null) return { axis: "vertical", index: v };
    const h = guideNear(view, guides, "horizontal", at[1]);
    return h === null ? null : { axis: "horizontal", index: h };
  };
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
  /** Ends the gesture the session is following, as a cancel; `release`
   * also lets go of its pointer (not when the press becomes a pinch's
   * first finger, which stays captured). */
  const cancel = (release = true) => {
    if (held === null) return;
    const id = held;
    held = null;
    if (release && element.hasPointerCapture(id)) element.releasePointerCapture(id);
    // A guide drag cut short keeps the guide where it is.
    if (guideDrag !== null) guideDrag = null;
    else send({ kind: "pointerCancel" });
  };
  /** Forgets touch points whose release this region will not see (a lost
   * capture, the window's blur), and the pinch once none is left. */
  const forget = (id: number | null) => {
    if (id === null) touches.clear();
    else touches.delete(id);
    if (touches.size === 0) pinch = null;
  };
  const blur = () => {
    cancel();
    forget(null);
  };

  const cursor = (to: Cursor | "col-resize" | "row-resize") => {
    element.style.cursor = to;
  };
  /** Stops drawing and says why; the panels stay usable. */
  const fail = (message: string) => {
    editor.loop.setPainter(null);
    editor.canvas = null;
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
    // In device pixels, so that a pixel-ratio change alone (a window moved
    // to another screen, the browser's zoom) is seen too.
    try {
      resize.observe(region, { box: "device-pixel-content-box" });
    } catch {
      resize.observe(region);
    }
    measure();
    // Where device pixels are not observed.
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
      rulers?.retheme();
    });
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    window.addEventListener("blur", blur);
    region.addEventListener("wheel", onWheel, { passive: false });
    onCleanup(() => {
      resize.disconnect();
      ratio?.removeEventListener("change", onRatio);
      theme.disconnect();
      window.removeEventListener("blur", blur);
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
        ...(editor.images === null
          ? {}
          : {
              images: (hash: string) => editor.images?.source(hash) ?? null,
              onImageFailed: (hash: string) => editor.images?.markFailed(hash),
            }),
      });
    } catch (error) {
      const panicked = error instanceof DesignerError && error.failure.kind === "panic";
      return setFailed(
        panicked ? msg("canvas.failed", { message: error.message }) : msg("canvas.noWebgl"),
      );
    }
    const c = canvas;
    editor.canvas = c;
    onCleanup(() => {
      if (editor.canvas === c) editor.canvas = null;
    });
    const unavailable = editor.images?.onAvailable(() => {
      if (c.disposed) return;
      c.retryImages();
      painter?.invalidate();
    });
    if (unavailable !== undefined) onCleanup(unavailable);
    painter = new Painter(
      {
        draw(time, overlay) {
          const again = c.draw(time, overlay);
          editor.setLimits(c.zoomLimits());
          return again;
        },
      },
      (names) => {
        if (names.has("cursor")) cursor(editor.session.cursor());
        // The readout lives in the overlay: read it only while a gesture or
        // a drawing tool may have one.
        if (names.has("overlay")) {
          const drawing = editor.views.history.gesture || editor.views.tool.tool !== "select";
          const o = drawing ? editor.run((s) => s.overlay()) : undefined;
          const shown = o !== undefined && (o.readout !== null || o.entry !== "");
          editor.setReadout(shown ? { readout: o.readout, entry: o.entry } : null);
        }
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
          // The rulers, in the same frame and from the same view.
          if (props.rulers) rulers?.frame();
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
    // The chart moved under a still pointer: the hovered seat may differ.
    if (over !== null) untrack(resample);
  });
  // Zoom to fit on the first measure, and only then.
  let fitted = false;
  createEffect(() => {
    if (fitted || editor.size()[0] <= 0) return;
    fitted = true;
    editor.frame(editor.run((s) => s.fit()) ?? null);
  });

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
    cancel(false);
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
          // A press on a user guide moves it; off the canvas it goes.
          const grabbed = e.button === 0 && !e.altKey ? guideAt(local(e)) : null;
          if (grabbed !== null) {
            guideDrag = grabbed;
            return;
          }
          send({ kind: "pointerDown", data: pointer(e) });
        }}
        onPointerMove={(e) => {
          if (touchMove(e)) return;
          if (held !== null && e.pointerId !== held) return;
          if (guideDrag !== null) return rulers?.dragTo(guideDrag, local(e));
          const p = pointer(e);
          editor.setPointer(p.world);
          send({ kind: "pointerMove", data: p });
          if (held === null) {
            const g = guideAt(p.screen);
            if (g !== null) cursor(g.axis === "vertical" ? "col-resize" : "row-resize");
            else if (element.style.cursor.endsWith("-resize")) cursor(editor.session.cursor());
          }
          over = p.screen;
          if (editor.views.numbering !== null) resample();
        }}
        onPointerUp={(e) => {
          if (touchUp(e) || e.pointerId !== held) return;
          held = null;
          if (guideDrag !== null) {
            rulers?.drop(guideDrag, local(e));
            guideDrag = null;
            return;
          }
          send({ kind: "pointerUp", data: pointer(e) });
        }}
        onPointerCancel={(e) => {
          touchUp(e);
          if (e.pointerId === held) cancel();
        }}
        // Capture lost while the press is held (the element hidden, another
        // capture): the gesture ends as a cancel, not half way.
        onLostPointerCapture={(e) => {
          // After a release the pointer is already forgotten; a touch still
          // tracked here was lost while down.
          if (touches.has(e.pointerId)) forget(e.pointerId);
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
      <Show when={props.rulers}>
        <Rulers editor={editor} guides={() => editor.views.guides} ref={(h) => (rulers = h)} />
      </Show>
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
