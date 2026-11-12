// The frame loop against a real session (the designer's WASM, from
// `cargo xtask build-designer`): it reads only the views `changed()` names,
// what it writes equals a fresh read, and validation runs only in idle time
// outside a gesture.
import { beforeAll, describe, expect, test } from "bun:test";

import { loadDesigner, type Designer, type Draft, type Session } from "@cavea/designer-core";
import { FrameLoop, type IdleDeadline, type Scheduler, type Views } from "./loop.ts";
import { emptyChart } from "./source.ts";

const wasm = new URL(
  "../../../../packages/designer-core/wasm/cavea_designer_bg.wasm",
  import.meta.url,
);
let designer: Designer;

beforeAll(async () => {
  const file = Bun.file(wasm);
  if (!(await file.exists())) {
    throw new Error("the designer WASM is missing: run `cargo xtask build-designer` first");
  }
  designer = await loadDesigner({ wasm: await file.arrayBuffer() });
});

/** A chart of `blocks` row blocks, the first two in a section. */
function chart(blocks: number): Session {
  const s = designer.open(emptyChart("Loop test"));
  const ids: number[] = [];
  for (let i = 0; i < blocks; i++) {
    const draft: Draft = {
      kind: "rowBlock",
      origin: [i * 2000, 0],
      rows: 4,
      seats: 10,
      seatPitch: 55,
      rowPitch: 95,
    };
    ids.push(s.insert(null, draft));
  }
  s.group(ids.slice(0, 2), null);
  return s;
}

/** Counts every session call by name. */
function counted(s: Session): { session: Session; calls: Map<string, number> } {
  const calls = new Map<string, number>();
  const session = new Proxy(s, {
    get(target, key, receiver) {
      const v: unknown = Reflect.get(target, key, receiver);
      if (typeof v !== "function" || typeof key !== "string") return v;
      return (...args: unknown[]) => {
        calls.set(key, (calls.get(key) ?? 0) + 1);
        return (v as (...a: unknown[]) => unknown).apply(target, args);
      };
    },
  });
  return { session, calls };
}

class Recorder {
  last: Partial<Views> = {};
  set<K extends keyof Views>(view: K, value: Views[K]): void {
    this.last[view] = value;
  }
}

/** Frames and idle callbacks run by hand, on a clock set by hand. */
function manual() {
  const idle: ((d: IdleDeadline) => void)[] = [];
  const clock = { now: 0 };
  const scheduler: Scheduler = {
    frame: () => () => {},
    idle(run) {
      idle.push(run);
      return () => idle.splice(idle.indexOf(run), 1);
    },
    now: () => clock.now,
    visibility: () => () => {},
    hidden: () => false,
  };
  const runIdle = (ms: number) => {
    const pending = idle.splice(0);
    for (const run of pending) run({ timeRemaining: () => ms });
    return pending.length;
  };
  return { scheduler, clock, runIdle };
}

describe("the frame loop", () => {
  test("reads every view once, then nothing until something changes", () => {
    const { session, calls } = counted(chart(3));
    const sink = new Recorder();
    const loop = new FrameLoop(session, sink, manual().scheduler);
    loop.setLayersWindow(0, 50);
    const all: (keyof Views)[] = [
      "categories",
      "floors",
      "history",
      "inspector",
      "layers",
      "selection",
      "status",
      "title",
      "tool",
      "validation",
      "guides",
    ];
    expect(new Set(loop.tick())).toEqual(new Set(all));
    calls.clear();
    for (let i = 0; i < 10; i++) expect(loop.tick()).toEqual([]);
    expect([...calls]).toEqual([["changed", 10]]);
  });

  test("re-reads only what changed, and what it holds equals a fresh read", () => {
    const raw = chart(3);
    const { session, calls } = counted(raw);
    const sink = new Recorder();
    const loop = new FrameLoop(session, sink, manual().scheduler);
    loop.setLayersWindow(0, 50);
    loop.tick();

    const fresh = (): Omit<Views, "numbering"> => ({
      history: raw.history(),
      title: raw.title(),
      floors: raw.floors(),
      status: raw.status(),
      selection: raw.selectionView(),
      tool: raw.toolView(),
      inspector: raw.inspector(),
      categories: raw.categories(),
      validation: raw.validation(),
      layers: raw.layers(0, 50),
      guides: raw.guides(),
    });

    // A selection change is not an edit: the title, floors and categories stay.
    const top = raw.layers(0, 50).rows.find((r) => r.kind === "rowBlock");
    raw.clickLayer(top?.id ?? -1, "replace");
    calls.clear();
    const read = loop.tick();
    expect(read).toContain("selection");
    expect(read).toContain("inspector");
    expect(read).not.toContain("title");
    expect(read).not.toContain("categories");
    expect(calls.get("title")).toBeUndefined();
    expect(sink.last).toEqual(fresh());

    // An edit, then a window that moves: each read once, nothing else.
    raw.setField({ field: "rowPitch", value: 100 });
    loop.tick();
    expect(sink.last).toEqual(fresh());
    loop.setLayersWindow(1, 2);
    expect(loop.tick()).toEqual(["layers"]);
    expect(sink.last.layers).toEqual(raw.layers(1, 2));
  });

  test("validates in idle time once edits settle, and never in a gesture", () => {
    const raw = chart(2);
    const { session, calls } = counted(raw);
    const sink = new Recorder();
    const { scheduler, clock, runIdle } = manual();
    const loop = new FrameLoop(session, sink, scheduler);
    loop.tick();
    expect(sink.last.validation?.complete).toBe(false);

    // Right after the edit nothing runs; the idle callback asks again.
    runIdle(50);
    expect(calls.get("validate")).toBeUndefined();

    // A scrub is a gesture: still nothing.
    raw.clickLayer(raw.layers(0, 10).rows.at(-1)?.id ?? -1, "replace");
    raw.beginScrub();
    loop.tick();
    clock.now += 5_000;
    runIdle(50);
    expect(calls.get("validate")).toBeUndefined();
    raw.endScrub();

    // Settled and outside the gesture: it runs to completion.
    loop.tick();
    for (let i = 0; i < 100 && runIdle(50) > 0; i++) loop.tick();
    loop.tick();
    expect(calls.get("validate")).toBeGreaterThan(0);
    expect(sink.last.validation).toEqual(raw.validation());
    expect(sink.last.validation?.complete).toBe(true);
  });
});
