// When the canvas draws, against a real session (the designer's WASM, from
// `cargo xtask build-designer`): in the frame after something changed or
// asked, and never while the designer is idle (07 §14).
import { beforeAll, describe, expect, test } from "bun:test";

import type { Designer, Draft } from "@cavea/designer-core";
import { FrameLoop, type Scheduler } from "./loop.ts";
import { Painter } from "./paint.ts";
import { emptyChart } from "./source.ts";
import { designer as load } from "./testkit.ts";

let designer: Designer;
beforeAll(async () => {
  designer = await load();
});

const scheduler: Scheduler = {
  frame: () => () => {},
  idle: () => () => {},
  now: () => 0,
  visibility: () => () => {},
  hidden: () => false,
};

const NONE = { shift: false, alt: false, ctrl: false };
const BLOCK: Draft = {
  kind: "rowBlock",
  origin: [0, 0],
  rows: 2,
  seats: 3,
  seatPitch: 55,
  rowPitch: 95,
};

/** A loop drawing into a fake canvas that asks for `again` more frames. */
function setup() {
  const session = designer.open(emptyChart("Paint test"));
  const loop = new FrameLoop(session, { set() {} }, scheduler);
  const canvas = { again: 0, draw: () => canvas.again-- > 0 };
  const painter = new Painter(canvas);
  loop.setPainter(painter);
  let time = 0;
  /** Runs `n` animation frames; returns the draws they made. */
  const frames = (n: number) => {
    const before = painter.draws;
    for (let i = 0; i < n; i++) loop.tick((time += 16));
    return painter.draws - before;
  };
  return { session, loop, canvas, painter, frames };
}

describe("the painter", () => {
  test("draws the first frame, then nothing while idle", () => {
    const { frames } = setup();
    expect(frames(1)).toBe(1);
    expect(frames(100)).toBe(0);
  });

  test("draws once after an edit, a selection or a pointer move", () => {
    const { session, frames } = setup();
    frames(1);
    session.insert(null, BLOCK);
    expect(frames(10)).toBe(1);
    // Escape deselects the new block.
    session.input({ kind: "keyDown", data: ["escape", NONE] });
    expect(frames(10)).toBe(1);
    session.input({
      kind: "pointerMove",
      data: {
        world: [5, 5],
        screen: [5, 5],
        zoom: 1,
        button: "primary",
        kind: "mouse",
        mods: NONE,
        time: 0,
      },
    });
    expect(frames(10)).toBe(1);
  });

  test("draws when the view changes, and while the canvas asks for frames", () => {
    const { canvas, painter, frames } = setup();
    frames(1);
    painter.invalidate();
    expect(frames(10)).toBe(1);
    canvas.again = 3;
    painter.invalidate();
    // The draw asked for, then the three the canvas wanted after it.
    expect(frames(10)).toBe(4);
  });

  test("a flush reads the change but leaves the draw to the next frame", () => {
    const { session, loop, frames, painter } = setup();
    frames(1);
    session.insert(null, BLOCK);
    loop.flush();
    expect(painter.draws).toBe(1);
    expect(frames(1)).toBe(1);
  });
});
