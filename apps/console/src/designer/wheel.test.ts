// The wheel and the pinch on the camera (07 §4): what a wheel event does,
// and that a pinch keeps the chart point between the fingers between them
// while it zooms and pans.
import { describe, expect, test } from "bun:test";

import { pan, toChart, zoomAbout, type View } from "@cavea/designer-core";
import { pinchStep, wheelStep, type Wheel } from "./wheel.ts";

const wheel = (w: Partial<Wheel>): Wheel => ({
  deltaX: 0,
  deltaY: 0,
  deltaMode: 0,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  ...w,
});

describe("the wheel", () => {
  test("pans by its travel in pixels, lines or pages; Shift turns it sideways", () => {
    expect(wheelStep(wheel({ deltaX: 3, deltaY: 40 }), 800)).toEqual({
      zoom: null,
      pan: [-3, -40],
    });
    expect(wheelStep(wheel({ deltaY: 3, deltaMode: 1 }), 800).pan).toEqual([-0, -48]);
    expect(wheelStep(wheel({ deltaY: 1, deltaMode: 2 }), 800).pan).toEqual([-0, -800]);
    expect(wheelStep(wheel({ deltaY: 40, shiftKey: true }), 800).pan).toEqual([-40, -0]);
  });

  test("zooms with Ctrl or Cmd: in for up, a mouse notch at most 1.35 times", () => {
    const notch = wheelStep(wheel({ deltaY: -100, ctrlKey: true }), 800).zoom ?? 0;
    expect(notch).toBeCloseTo(Math.exp(0.3), 6);
    expect(wheelStep(wheel({ deltaY: 100, metaKey: true }), 800).zoom).toBeCloseTo(1 / notch, 6);
    // A pinch's small steps zoom in proportion.
    expect(wheelStep(wheel({ deltaY: -2, ctrlKey: true }), 800).zoom).toBeCloseTo(
      Math.exp(0.02),
      6,
    );
  });
});

/** A seeded generator, so a failure repeats. */
function random(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s * 1_103_515_245 + 12_345) % 2_147_483_648;
    return s / 2_147_483_648;
  };
}

describe("the pinch", () => {
  test("keeps the chart point between the fingers between them", () => {
    const r = random(7);
    for (let i = 0; i < 500; i++) {
      let view: View = {
        centre: [r() * 4000 - 2000, r() * 4000 - 2000],
        zoom: 0.05 + r() * 2,
        width: 1100,
        height: 800,
        dpr: 1,
      };
      const pt = (): [number, number] => [r() * 1100, r() * 800];
      const from: [[number, number], [number, number]] = [pt(), pt()];
      const to: [[number, number], [number, number]] = [pt(), pt()];
      const mid = (p: typeof from): [number, number] => [
        (p[0][0] + p[1][0]) / 2,
        (p[0][1] + p[1][1]) / 2,
      ];
      const held = toChart(view, mid(from));
      const step = pinchStep({ points: from }, to);
      // As the canvas region applies it: the pan, then the zoom.
      view = zoomAbout(pan(view, step.pan), step.factor, step.at);
      const now = toChart(view, mid(to));
      expect(now[0]).toBeCloseTo(held[0], 6);
      expect(now[1]).toBeCloseTo(held[1], 6);
    }
  });

  test("two fingers on one point neither zoom nor jump", () => {
    const step = pinchStep(
      {
        points: [
          [10, 10],
          [10, 10],
        ],
      },
      [
        [10, 10],
        [10, 10],
      ],
    );
    expect(step).toEqual({ pan: [0, 0], factor: 1, at: [10, 10] });
  });
});
