// The wheel and the pinch as camera steps (07 §4): the wheel pans by its
// travel, Ctrl or Cmd with it zooms (a trackpad's pinch arrives that way),
// and two fingers zoom about their midpoint and pan by its travel.

/** The wheel event fields a step reads. */
export interface Wheel {
  deltaX: number;
  deltaY: number;
  /** 0 pixels, 1 lines, 2 pages. */
  deltaMode: number;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}

/** CSS px per wheel line (Firefox reports lines). */
const LINE = 16;
/** A wheel zoom step is at most this much travel: a mouse notch (about 100
 * px) zooms by 1.35, a pinch's small steps in proportion. */
const ZOOM_TRAVEL = 30;

/**
 * What a wheel event does: `zoom`, a factor about the pointer, with Ctrl or
 * Cmd; else `pan`, the chart moved by that many CSS px (it scrolls with
 * the wheel: down shows what is below). Shift turns a vertical wheel
 * sideways. `page` is the canvas's height, for page steps.
 */
export function wheelStep(
  e: Wheel,
  page: number,
): { zoom: number; pan: null } | { zoom: null; pan: [number, number] } {
  const unit = e.deltaMode === 1 ? LINE : e.deltaMode === 2 ? page : 1;
  let dx = e.deltaX * unit;
  let dy = e.deltaY * unit;
  if (e.ctrlKey || e.metaKey) {
    const travel = Math.min(Math.max(dy, -ZOOM_TRAVEL), ZOOM_TRAVEL);
    return { zoom: Math.exp(-travel / 100), pan: null };
  }
  if (e.shiftKey && dx === 0) [dx, dy] = [dy, 0];
  return { zoom: null, pan: [-dx, -dy] };
}

/** Two touch points, in CSS px on the canvas. */
export interface Pinch {
  points: [[number, number], [number, number]];
}

/**
 * From one pair of touch points to the next: the chart pans by the
 * midpoint's travel, then zooms by the change of distance about the new
 * midpoint, so the chart point between the fingers stays between them.
 */
export function pinchStep(
  from: Pinch,
  to: Pinch["points"],
): { pan: [number, number]; factor: number; at: [number, number] } {
  const mid = ([a, b]: Pinch["points"]): [number, number] => [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
  const span = ([a, b]: Pinch["points"]) => Math.hypot(b[0] - a[0], b[1] - a[1]);
  const [m0, m1] = [mid(from.points), mid(to)];
  const [d0, d1] = [span(from.points), span(to)];
  return {
    pan: [m1[0] - m0[0], m1[1] - m0[1]],
    factor: d0 > 0 && d1 > 0 ? d1 / d0 : 1,
    at: m1,
  };
}
