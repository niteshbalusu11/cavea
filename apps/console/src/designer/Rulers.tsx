// The rulers along the canvas's top and left edges (07 §5.2, DSN-031): the
// chart's units at the canvas's own view, the selection's extent, and the
// floor's user guides. They are drawn in the canvas's frame, from the same
// view, so they never trail it, and only when the view, the theme, the
// guides or the selection changed: an idle designer draws nothing.
//
// A guide is dragged out of a ruler onto the canvas, moved by dragging it
// on the canvas or its mark on the ruler, and removed by dragging it off
// the canvas. Without a pointer: focus a ruler, Enter adds a guide at the
// view's centre, the arrows along the ruler move the current guide (1 unit,
// Shift 10), the arrows across it choose another, Delete removes it.
import { toChart, toScreen, type UserGuide, type View } from "@cavea/designer-core";
import { createEffect, createSignal, on, type Accessor } from "solid-js";
import { formatNumber, msg } from "../i18n/messages.ts";
import type { Editor } from "./editor.ts";
import { step, ticks } from "./ruler.ts";

/** The rulers' thickness, CSS px. */
export const RULER = 20;
/** A guide is taken by a press this close to it, CSS px. */
export const GUIDE_REACH = 4;

type Axis = "vertical" | "horizontal";

interface Palette {
  bg: string;
  line: string;
  tick: string;
  text: string;
  extent: string;
  guide: string;
  focus: string;
}

function palette(): Palette {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return {
    bg: v("--c-app"),
    line: v("--c-line-strong"),
    tick: v("--c-faint"),
    text: v("--c-muted"),
    extent: v("--c-accent-soft"),
    guide: v("--c-guide"),
    focus: v("--c-focus"),
  };
}

/** A guide's position on the canvas along its ruler, CSS px. */
const screenOf = (view: View, g: UserGuide): number =>
  g.axis === "vertical" ? toScreen(view, [g.at, 0])[0] : toScreen(view, [0, g.at])[1];

/** The index of the guide of `axis` nearest `px` along its ruler within
 * `GUIDE_REACH`, or `null`. */
export function guideNear(
  view: View,
  guides: readonly UserGuide[],
  axis: Axis,
  px: number,
): number | null {
  let best: number | null = null;
  let distance: number = GUIDE_REACH;
  guides.forEach((g, i) => {
    if (g.axis !== axis) return;
    const d = Math.abs(screenOf(view, g) - px);
    if (d <= distance) [best, distance] = [i, d];
  });
  return best;
}

/** Quarter units close in, whole units otherwise: what a guide snaps to. */
const round = (at: number, zoom: number) => (zoom >= 4 ? Math.round(at * 4) / 4 : Math.round(at));

export interface GuideDrag {
  axis: Axis;
  /** The guide's index once it exists. */
  index: number | null;
}

export interface RulersHandle {
  /** Draws if anything shown changed. Called in the canvas's frame. */
  frame(): void;
  /** A guide drag from the canvas or a ruler, at a point of the canvas
   * region (CSS px). */
  dragTo(drag: GuideDrag, at: [number, number]): void;
  /** Ends a drag: off the canvas, the guide goes. */
  drop(drag: GuideDrag, at: [number, number]): void;
  /** The theme changed: colours are read again. */
  retheme(): void;
}

export function Rulers(props: {
  editor: Editor;
  guides: Accessor<UserGuide[]>;
  ref: (handle: RulersHandle) => void;
}) {
  const editor = props.editor;
  let top!: HTMLCanvasElement;
  let left!: HTMLCanvasElement;
  let colours: Palette | null = null;
  let drawn = "";
  /** The guide the keys act on, per ruler. */
  const [current, setCurrent] = createSignal<{ axis: Axis; index: number } | null>(null);
  const [focused, setFocused] = createSignal<Axis | null>(null);
  const [said, setSaid] = createSignal("");
  // Indices are the floor shown's: another floor's guides are others.
  createEffect(
    on(
      () => editor.views.floors.shown,
      () => setCurrent(null),
      { defer: true },
    ),
  );

  const inside = (at: [number, number]): boolean => {
    const [w, h] = editor.size();
    return at[0] >= RULER && at[1] >= RULER && at[0] <= w && at[1] <= h;
  };

  const handle: RulersHandle = {
    frame() {
      const view = editor.view();
      const bounds = editor.views.selection.bounds;
      const key = JSON.stringify([view, bounds, props.guides(), current(), focused()]);
      if (key === drawn || view.width <= 0) return;
      drawn = key;
      colours ??= palette();
      draw(top, "vertical", view, colours, props.guides(), bounds, current(), focused());
      draw(left, "horizontal", view, colours, props.guides(), bounds, current(), focused());
    },
    dragTo(drag, at) {
      if (!inside(at)) return;
      const view = editor.view();
      const world = toChart(view, at);
      const value = round(drag.axis === "vertical" ? world[0] : world[1], view.zoom);
      if (drag.index === null) {
        const index = editor.run((s) => s.addGuide({ axis: drag.axis, at: value }));
        if (index !== undefined) drag.index = index;
      } else {
        const index = drag.index;
        editor.run((s) => s.moveGuide(index, value));
      }
      if (drag.index !== null) setCurrent({ axis: drag.axis, index: drag.index });
    },
    drop(drag, at) {
      if (drag.index === null || inside(at)) return;
      const index = drag.index;
      editor.run((s) => s.removeGuide(index));
      setCurrent(null);
    },
    retheme() {
      colours = null;
      drawn = "";
    },
  };
  props.ref(handle);

  /** The guides of `axis`, by position, with their indices. */
  const ofAxis = (axis: Axis) => {
    const list = props
      .guides()
      .map((g, index) => ({ ...g, index }))
      .filter((g) => g.axis === axis);
    // oxlint-disable-next-line unicorn/no-array-sort -- `list` is ours; `toSorted` is ES2023, past the lib (ES2022).
    return list.sort((a, b) => a.at - b.at);
  };

  const describe = (axis: Axis, index: number | null) => {
    const g = index === null ? undefined : props.guides()[index];
    setSaid(
      g === undefined
        ? msg("guide.none")
        : msg(axis === "vertical" ? "guide.vertical" : "guide.horizontal", {
            at: formatNumber(g.at),
          }),
    );
  };

  const onKey = (axis: Axis) => (e: KeyboardEvent) => {
    const along = axis === "vertical" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
    const across = axis === "vertical" ? ["ArrowUp", "ArrowDown"] : ["ArrowLeft", "ArrowRight"];
    const c = current();
    const mine = c !== null && c.axis === axis && props.guides()[c.index]?.axis === axis ? c : null;
    if (e.key === "Enter" || e.key === "Insert") {
      const view = editor.view();
      const at = round(axis === "vertical" ? view.centre[0] : view.centre[1], view.zoom);
      const index = editor.run((s) => s.addGuide({ axis, at }));
      if (index === undefined) return;
      setCurrent({ axis, index });
      editor.loop.flush();
      describe(axis, index);
    } else if (along.includes(e.key) && mine !== null) {
      const g = props.guides()[mine.index];
      if (g === undefined) return;
      const by = (e.key === along[0] ? -1 : 1) * (e.shiftKey ? 10 : 1);
      editor.run((s) => s.moveGuide(mine.index, g.at + by));
      editor.loop.flush();
      describe(axis, mine.index);
    } else if (across.includes(e.key)) {
      const list = ofAxis(axis);
      if (list.length === 0) return describe(axis, null);
      const at = mine === null ? -1 : list.findIndex((g) => g.index === mine.index);
      const next =
        list[Math.max(0, Math.min(list.length - 1, at + (e.key === across[0] ? -1 : 1)))];
      if (next === undefined) return;
      setCurrent({ axis, index: next.index });
      describe(axis, next.index);
    } else if ((e.key === "Delete" || e.key === "Backspace") && mine !== null) {
      editor.run((s) => s.removeGuide(mine.index));
      setCurrent(null);
      editor.loop.flush();
      describe(axis, null);
    } else {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  /** A press on a ruler: its guide's mark moves that guide, anywhere else
   * a new guide is dragged out. */
  const onPress = (axis: Axis) => (e: PointerEvent) => {
    if (e.button !== 0) return;
    const el = e.currentTarget as HTMLCanvasElement;
    const region = el.parentElement?.getBoundingClientRect();
    if (region === undefined) return;
    const at = (ev: PointerEvent): [number, number] => [
      ev.clientX - region.left,
      ev.clientY - region.top,
    ];
    const p = at(e);
    const along = axis === "vertical" ? p[0] : p[1];
    const drag: GuideDrag = { axis, index: guideNear(editor.view(), props.guides(), axis, along) };
    el.setPointerCapture(e.pointerId);
    e.preventDefault();
    const move = (ev: PointerEvent) => handle.dragTo(drag, at(ev));
    const up = (ev: PointerEvent) => {
      handle.drop(drag, at(ev));
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  const ruler = (axis: Axis) =>
    `absolute bg-app outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus ${
      axis === "vertical" ? "cursor-col-resize" : "cursor-row-resize"
    }`;
  return (
    <>
      <canvas
        ref={(e) => (top = e)}
        tabIndex={0}
        aria-label={msg("ruler.top")}
        aria-describedby="ruler-said"
        class={ruler("vertical")}
        style={{ left: `${RULER}px`, top: "0", right: "0", height: `${RULER}px` }}
        onPointerDown={onPress("vertical")}
        onKeyDown={onKey("vertical")}
        onFocus={() => setFocused("vertical")}
        onBlur={() => setFocused(null)}
      />
      <canvas
        ref={(e) => (left = e)}
        tabIndex={0}
        aria-label={msg("ruler.left")}
        aria-describedby="ruler-said"
        class={ruler("horizontal")}
        style={{ left: "0", top: `${RULER}px`, bottom: "0", width: `${RULER}px` }}
        onPointerDown={onPress("horizontal")}
        onKeyDown={onKey("horizontal")}
        onFocus={() => setFocused("horizontal")}
        onBlur={() => setFocused(null)}
      />
      <div
        aria-hidden="true"
        class="absolute top-0 left-0 border-r border-b border-line-strong bg-app"
        style={{ width: `${RULER}px`, height: `${RULER}px` }}
      />
      <p id="ruler-said" class="sr-only" aria-live="polite">
        {said()}
      </p>
    </>
  );
}

const tickFormats = new Map<number, Intl.NumberFormat>();
/** A tick's label: grouped as the page's numbers are, with the step's
 * decimals. */
function formatTick(value: number, decimals: number): string {
  let f = tickFormats.get(decimals);
  if (f === undefined) {
    f = new Intl.NumberFormat("en", { maximumFractionDigits: decimals });
    tickFormats.set(decimals, f);
  }
  return f.format(value);
}

/** One ruler: ticks and labels in chart units, the selection's extent, the
 * guides of its axis (the current one stronger), its inner edge. */
function draw(
  canvas: HTMLCanvasElement,
  axis: Axis,
  view: View,
  c: Palette,
  guides: readonly UserGuide[],
  bounds: { min: [number, number]; max: [number, number] } | null,
  current: { axis: Axis; index: number } | null,
  focused: Axis | null,
): void {
  const horizontalRuler = axis === "vertical";
  const length = (horizontalRuler ? view.width : view.height) - RULER;
  const dpr = view.dpr;
  const w = Math.max(1, Math.round((horizontalRuler ? length : RULER) * dpr));
  const h = Math.max(1, Math.round((horizontalRuler ? RULER : length) * dpr));
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (ctx === null) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = c.bg;
  ctx.fillRect(0, 0, w, h);
  // Along the ruler, from its start (the canvas region's RULER px).
  const i = horizontalRuler ? 0 : 1;
  const pos = (v: number) => {
    const p: [number, number] = horizontalRuler ? [v, 0] : [0, v];
    return toScreen(view, p)[i] - RULER;
  };
  /** A label beside its tick: along the top ruler, and reading upwards
   * along the left one. */
  const label = (p: number, text: string) => {
    ctx.save();
    if (horizontalRuler) ctx.translate(p, 0);
    else {
      ctx.translate(0, p);
      ctx.rotate(-Math.PI / 2);
    }
    ctx.fillText(text, 3, 2);
    ctx.restore();
  };
  // The selection's extent.
  if (bounds !== null) {
    const a = pos(bounds.min[i] ?? 0);
    const b = pos(bounds.max[i] ?? 0);
    ctx.fillStyle = c.extent;
    if (horizontalRuler) ctx.fillRect(a, 0, Math.max(1, b - a), RULER);
    else ctx.fillRect(0, a, RULER, Math.max(1, b - a));
  }
  const s = step(view.zoom);
  const from = toChart(view, [RULER, RULER])[i] ?? 0;
  const to = toChart(view, [view.width, view.height])[i] ?? 0;
  ctx.strokeStyle = c.tick;
  ctx.lineWidth = 1;
  ctx.beginPath();
  const labels: [number, string][] = [];
  for (const t of ticks(from, to, s, formatTick)) {
    const p = Math.round(pos(t.at)) + 0.5;
    const len = t.label === null ? 5 : RULER;
    if (horizontalRuler) {
      ctx.moveTo(p, RULER);
      ctx.lineTo(p, RULER - len);
    } else {
      ctx.moveTo(RULER, p);
      ctx.lineTo(RULER - len, p);
    }
    if (t.label !== null) labels.push([p, t.label]);
  }
  ctx.stroke();
  ctx.fillStyle = c.text;
  ctx.font = "10px system-ui, sans-serif";
  ctx.textBaseline = "top";
  for (const [p, text] of labels) label(p, text);
  // The guides of this ruler's axis: a mark each.
  guides.forEach((g, index) => {
    if (g.axis !== axis) return;
    const p = Math.round(pos(g.at)) + 0.5;
    const strong = current?.axis === axis && current.index === index && focused === axis;
    ctx.fillStyle = strong ? c.focus : c.guide;
    ctx.beginPath();
    if (horizontalRuler) {
      ctx.moveTo(p - 5, RULER - 8);
      ctx.lineTo(p + 5, RULER - 8);
      ctx.lineTo(p, RULER);
    } else {
      ctx.moveTo(RULER - 8, p - 5);
      ctx.lineTo(RULER - 8, p + 5);
      ctx.lineTo(RULER, p);
    }
    ctx.fill();
  });
  // The inner edge.
  ctx.fillStyle = c.line;
  if (horizontalRuler) ctx.fillRect(0, RULER - 1, length, 1);
  else ctx.fillRect(RULER - 1, 0, 1, length);
}
