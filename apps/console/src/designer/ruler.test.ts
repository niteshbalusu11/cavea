// The rulers' ticks at every zoom the designer allows.
import { expect, test } from "bun:test";
import { MIN_LABEL_PX, step, ticks } from "./ruler.ts";

const format = (v: number, decimals: number) => v.toFixed(decimals);

test("labelled ticks are 1, 2 or 5 × 10^k apart, 64 to 160 px, at every zoom", () => {
  // Every zoom from 0.002 to 64 px per unit, finely and at the ends.
  const zooms = [0.002, 64, 1, 0.1, 6.4];
  for (let z = Math.log(0.002); z <= Math.log(64); z += 0.013) zooms.push(Math.exp(z));
  for (const zoom of zooms) {
    const s = step(zoom);
    const px = s.major * zoom;
    expect(px).toBeGreaterThanOrEqual(MIN_LABEL_PX * (1 - 1e-9));
    expect(px).toBeLessThan(MIN_LABEL_PX * 2.5 + 1e-6);
    const mantissa = s.major / 10 ** Math.floor(Math.log10(s.major) + 1e-9);
    expect([1, 2, 5].some((m) => Math.abs(mantissa - m) < 1e-9)).toBe(true);
    expect(Number.isInteger(Math.round(s.major / s.minor))).toBe(true);
    expect(s.minor * zoom).toBeGreaterThanOrEqual(MIN_LABEL_PX / 5 - 1e-9);

    // A screen's worth of ticks anywhere on the chart, even far from 0:
    // evenly spaced, labels distinct, each label's value the tick's.
    for (const centre of [0, -12_345.67, 3e6]) {
      const half = 800 / zoom;
      const list = ticks(centre - half, centre + half, s, format);
      expect(list.length).toBeGreaterThan(0);
      expect(list.length).toBeLessThan(2 * 160);
      const labels = list.filter((t) => t.label !== null).map((t) => t.label);
      expect(new Set(labels).size).toBe(labels.length);
      for (const t of list) {
        if (t.label !== null) expect(Math.abs(Number(t.label) - t.at)).toBeLessThan(s.minor / 2);
      }
      for (let i = 1; i < list.length; i++) {
        const gap = (list[i]?.at ?? 0) - (list[i - 1]?.at ?? 0);
        expect(Math.abs(gap - s.minor)).toBeLessThan(s.minor * 1e-6);
      }
    }
  }
});

test("small steps have the decimals they need, and no label is -0", () => {
  expect(step(64)).toMatchObject({ major: 1, decimals: 0 });
  expect(step(100)).toMatchObject({ major: 1, decimals: 0 });
  expect(step(200)).toMatchObject({ major: 0.5, decimals: 1 });
  const list = ticks(-1, 1, step(200), format);
  expect(list.find((t) => t.at === 0 || Object.is(t.at, -0))?.label).toBe("0.0");
});
