// The rulers' arithmetic (07 §5.2): which ticks to draw along an edge of
// the canvas, in the chart's units, at the view's zoom. Labelled ticks are
// 1, 2 or 5 times a power of ten apart, the smallest such step at least
// `MIN_LABEL_PX` CSS pixels apart, so labels never crowd and never thin out
// by more than 2.5 times, from the designer's widest zoom (0.002 px per
// chart unit) to its narrowest (64).

/** CSS pixels between labelled ticks, at least. */
export const MIN_LABEL_PX = 64;

export interface Step {
  /** Chart units between labelled ticks: 1, 2 or 5 × 10^k. */
  major: number;
  /** Chart units between ticks; `major / minor` is a whole number. */
  minor: number;
  /** Minor ticks per labelled one. */
  per: number;
  /** Decimals a label needs: 0 from a step of 1 up. */
  decimals: number;
}

/** The power of ten exactly, not 10^k's rounding: a step of 0.1 is 1 / 10. */
const pow = (e: number) => (e >= 0 ? 10 ** e : 1 / 10 ** -e);

/** The steps for `zoom` CSS pixels per chart unit. */
export function step(zoom: number, minPx = MIN_LABEL_PX): Step {
  const raw = minPx / zoom;
  let k = Math.floor(Math.log10(raw));
  for (;;) {
    for (const [m, per] of [
      [1, 5],
      [2, 4],
      [5, 5],
    ] as const) {
      const major = m * pow(k);
      if (major >= raw * (1 - 1e-12)) {
        const decimals = Math.max(0, -k);
        return { major, minor: major / per, per, decimals };
      }
    }
    k += 1;
  }
}

export interface Tick {
  /** The chart coordinate. */
  at: number;
  /** Labelled, with this text; `null` for a minor tick. */
  label: string | null;
}

/**
 * The ticks between chart coordinates `from` and `to` (the visible range of
 * one axis), with their labels written by `format`. Counted from whole
 * multiples of the minor step, so a long range accumulates no error.
 */
export function ticks(
  from: number,
  to: number,
  s: Step,
  format: (value: number, decimals: number) => string,
): Tick[] {
  const out: Tick[] = [];
  const first = Math.ceil(from / s.minor);
  const last = Math.floor(to / s.minor);
  // A bound on the work whatever the input: a ruler a few thousand pixels
  // long has at most a few hundred ticks.
  if (!(last - first < 10_000)) return out;
  for (let i = first; i <= last; i++) {
    const at = i * s.minor;
    const major = ((i % s.per) + s.per) % s.per === 0;
    // `|| 0`: no "-0".
    const value = Number(at.toFixed(s.decimals)) || 0;
    out.push({ at, label: major ? format(value, s.decimals) : null });
  }
  return out;
}
