// The canvas region: the one boundary the canvas (DSN-004, DSN-005) fills.
// It owns nothing of the chart's drawing. It measures itself into
// `editor.setSize`, keeps focus for the canvas's keys, converts pointer
// events into the session's `Pointer` (world and screen coordinates at the
// camera's zoom) and sends them to `session.input`, which pans the camera
// through `editor.input`. The canvas builder replaces the placeholder with
// a `<canvas>` drawing `overlay()` at `editor.camera()`.
import type { Button, Pointer, PointerKind } from "@cavea/designer-core";
import { createEffect, on, onCleanup, onMount } from "solid-js";
import { formatNumber, msg } from "../i18n/messages.ts";
import type { Camera, Editor } from "./editor.ts";
import { modifiers } from "./keys.ts";

/** A point in the region (CSS px from its top left) in chart units. */
export function toWorld(
  camera: Camera,
  size: [number, number],
  screen: [number, number],
): [number, number] {
  return [
    camera.x + (screen[0] - size[0] / 2) / camera.zoom,
    camera.y + (screen[1] - size[1] / 2) / camera.zoom,
  ];
}

const BUTTONS: Record<number, Button> = { 0: "primary", 1: "middle", 2: "secondary" };

export function CanvasHost(props: { editor: Editor }) {
  const editor = props.editor;
  let el = document.createElement("div");

  const pointer = (e: PointerEvent): Pointer => {
    const r = el.getBoundingClientRect();
    const screen: [number, number] = [e.clientX - r.left, e.clientY - r.top];
    return {
      world: toWorld(editor.camera(), editor.size(), screen),
      screen,
      zoom: editor.camera().zoom,
      button: BUTTONS[e.button] ?? "primary",
      kind: (["mouse", "touch", "pen"].includes(e.pointerType)
        ? e.pointerType
        : "mouse") as PointerKind,
      mods: modifiers(e),
      time: e.timeStamp,
    };
  };

  onMount(() => {
    const resize = new ResizeObserver(([entry]) => {
      if (entry !== undefined) editor.setSize([entry.contentRect.width, entry.contentRect.height]);
    });
    resize.observe(el);
    onCleanup(() => resize.disconnect());
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

  return (
    <div
      ref={(e) => (el = e)}
      data-canvas
      tabIndex={0}
      role="application"
      aria-label={msg("canvas.label")}
      class="relative min-w-0 flex-1 touch-none overflow-hidden bg-canvas outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus"
      onPointerDown={(e) => {
        el.focus();
        el.setPointerCapture(e.pointerId);
        editor.input({ kind: "pointerDown", data: pointer(e) });
      }}
      onPointerMove={(e) => {
        const p = pointer(e);
        editor.setPointer(p.world);
        editor.input({ kind: "pointerMove", data: p });
      }}
      onPointerLeave={() => editor.setPointer(null)}
      onPointerUp={(e) => editor.input({ kind: "pointerUp", data: pointer(e) })}
      onPointerCancel={() => editor.input({ kind: "pointerCancel" })}
      onContextMenu={(e) => e.preventDefault()}
      onWheel={(e) => {
        e.preventDefault();
        if (e.ctrlKey || e.metaKey) editor.zoomBy(Math.exp(-e.deltaY / 300));
        else pan(editor, e.deltaX, e.deltaY);
      }}
    >
      <div class="pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-1 text-center select-none">
        <p class="text-lg font-medium text-fg">{editor.views.title}</p>
        <p class="text-base text-muted">
          {msg("canvas.summary", {
            seats: editor.views.status.seats,
            nodes: editor.views.status.nodes,
          })}
        </p>
        <p class="mt-3 text-sm text-faint">{msg("canvas.placeholder")}</p>
        <p class="text-xs text-faint tabular-nums">
          {msg("canvas.camera", {
            x: formatNumber(Math.round(editor.camera().x)),
            y: formatNumber(Math.round(editor.camera().y)),
            zoom: `${formatNumber(Math.round(editor.camera().zoom * 1000) / 10)}%`,
          })}
        </p>
      </div>
    </div>
  );
}

/** A wheel or trackpad scroll pans the camera by its travel. */
function pan(editor: Editor, dx: number, dy: number): void {
  const c = editor.camera();
  editor.setCamera({ ...c, x: c.x + dx / c.zoom, y: c.y + dy / c.zoom });
}
