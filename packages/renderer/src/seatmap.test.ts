// The SDK against a recording fake of the WASM view and a fake platform: no
// browser, no WebGL. What is tested is the SDK's logic, not the fake.
import { afterEach, describe, expect, spyOn, test } from "bun:test";
import {
  customState,
  staticState,
  type AdapterContext,
  type HoldResult,
  type StateAdapter,
} from "./adapter.ts";
import { CaveaError, warner } from "./errors.ts";
import { deferred, fakeEnv, flush, loaded, seat, sleep } from "./fakes.ts";
import { validateOptions, type Metrics, type SeatMapOptions } from "./options.ts";
import { createMap } from "./seatmap.ts";
const env = process.env as Record<string, string | undefined>;
afterEach(() => {
  delete env["NODE_ENV"];
});

describe("options", () => {
  test("each invalid option is rejected with its name", () => {
    const state = staticState();
    const cases: Array<[string, Record<string, unknown>]> = [
      ["options", 5 as unknown as Record<string, unknown>],
      ["chart", { chart: {} }],
      ["state", { state: { connect: state.connect } }],
      ["state", { state: { ...state, hold: () => Promise.resolve({ ok: true }) } }],
      ["selection.max", { selection: { max: 0 } }],
      ["selection.max", { selection: { max: 101 } }],
      ["selection.max", { selection: { max: 1.5 } }],
      ["selection.min", { selection: { max: 2, min: 3 } }],
      ["gestureHandling", { gestureHandling: "sideways" }],
      ["initialView", { initialView: { x: 0, y: 0, width: 0, height: 1 } }],
      ["maxDpr", { maxDpr: -1 }],
      ["reducedMotion", { reducedMotion: "sometimes" }],
      ["theme", { theme: "blue" }],
      ["theme", { theme: { tokens: { bg: {} } } }],
      ["assets", { assets: { wasm: 5 } }],
      ["onReady", { onReady: "yes" }],
    ];
    for (const [option, options] of cases) {
      let error: unknown;
      try {
        validateOptions(options, () => {});
      } catch (e) {
        error = e;
      }
      expect(error).toBeInstanceOf(CaveaError);
      expect(error).toMatchObject({ code: "invalid_option", fatal: true, option });
    }
  });

  test("unknown and unsupported options warn once each", () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    const once = warner(true);
    const options = {
      worker: true,
      colour: "red",
      mode: "static",
      selection: { validators: {}, maxx: 1 },
    };
    validateOptions(options, once);
    validateOptions(options, once);
    expect(warn.mock.calls.map((c) => c[0])).toEqual([
      '[cavea] option "worker" is not supported yet and is ignored',
      '[cavea] unknown option "colour" is ignored',
      '[cavea] mode "static" is not supported yet; the map runs in picker mode',
      '[cavea] option "selection.validators" is not supported yet and is ignored',
      '[cavea] unknown option "selection.maxx" is ignored',
    ]);
    warn.mockRestore();
  });

  test("warnings are on in development and off in production builds", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    for (const [mode, count] of [
      ["development", 1],
      ["production", 0],
    ] as const) {
      env["NODE_ENV"] = mode;
      warn.mockClear();
      const { map } = await loaded({ worker: true } as SeatMapOptions);
      expect(warn).toHaveBeenCalledTimes(count);
      map.destroy();
    }
    warn.mockRestore();
  });

  test("an invalid option rejects before anything is fetched", async () => {
    const f = fakeEnv();
    const promise = createMap(f.env, {} as HTMLElement, { maxDpr: 0 });
    await expect(promise).rejects.toMatchObject({ code: "invalid_option", option: "maxDpr" });
    expect(f.started).toEqual([]);
  });
});

describe("load sequence", () => {
  test("starts the four fetches together and draws before state and atlas arrive", async () => {
    const f = fakeEnv();
    const state = deferred<{ kind: "map"; unavailable: string[] }>();
    let connected = false;
    const adapter = customState({
      connect: () => {
        connected = true;
        return state.promise;
      },
      disconnect() {},
    });
    let done = false;
    const promise = createMap(f.env, {} as HTMLElement, {
      chart: { url: "c.ccf" },
      state: adapter,
    });
    void promise.then(() => (done = true));
    expect([...f.started, connected]).toEqual(["wasm", "chart", "atlas", true]);

    f.load();
    await flush();
    // Chart loaded and the frame loop attached; state and atlas still pending.
    expect(f.view.names()).toContain("loadChart");
    expect(f.attached()).toBeDefined();
    expect(f.view.names()).not.toContain("setSnapshotLabels");
    expect(done).toBe(false);

    state.resolve({ kind: "map", unavailable: ["A-1-1", "A-1-2"] });
    await promise;
    expect(f.view.calls).toContainEqual(["setSnapshotLabels", 1, ["A-1-1", "A-1-2"]]);
  });

  test("state that arrives before the chart is applied after it, in order", async () => {
    const f = fakeEnv();
    const adapter = customState({
      connect(ctx) {
        ctx.applyDelta({ unavailable: ["A-1-3"] });
        ctx.setMine(["A-1-1"], 99);
        return Promise.resolve({ kind: "binary", bytes: new Uint8Array([2, 0]) });
      },
      disconnect() {},
    });
    const promise = createMap(f.env, {} as HTMLElement, {
      chart: { bytes: new Uint8Array([1]) },
      state: adapter,
    });
    await flush();
    f.wasm.resolve(() => f.view.engine);
    await promise;
    expect(
      f.view
        .names()
        .filter((n) => !["setRules", "setManualSession", "setTheme", "setMessage"].includes(n)),
    ).toEqual(["loadChart", "applyFrame", "applyDeltaLabels", "setPrivateLabels"]);
  });

  test("a failed WASM, view, chart or connect rejects and tears down; a failed atlas does not", async () => {
    const cases: Array<[string, (f: ReturnType<typeof fakeEnv>) => void]> = [
      ["wasm_unavailable", (f) => f.wasm.reject(new Error("compile"))],
      [
        "webgl_unavailable",
        (f) => {
          f.state.makeView = () => {
            throw new Error("WebGL2 is not available");
          };
          f.load();
        },
      ],
      [
        "chart_fetch_failed",
        (f) => {
          f.wasm.resolve(() => f.view.engine);
          f.chart.reject(new Error("HTTP 404"));
        },
      ],
      // While the WASM is still loading, and with the adapter's own CaveaError.
      ["state_connect_failed", () => {}],
      // A first state the engine refuses.
      [
        "refused",
        (f) => {
          f.view.answers["setSnapshotLabels"] = () => {
            throw new Error("InvalidInput: snapshot length");
          };
          f.load();
        },
      ],
    ];
    for (const [code, fail] of cases) {
      const f = fakeEnv();
      let disconnected = 0;
      const adapter = customState({
        connect: () =>
          code === "state_connect_failed"
            ? Promise.reject(new CaveaError("state_failed", "503"))
            : code === "refused"
              ? Promise.resolve({ kind: "map", unavailable: [] })
              : new Promise(() => {}),
        disconnect: () => (disconnected += 1),
      });
      const promise = createMap(f.env, {} as HTMLElement, {
        chart: { url: "c.ccf" },
        state: adapter,
        // Without the built-in error state, a chart that fails rejects (05 §9).
        ui: { loading: false },
      });
      fail(f);
      await expect(promise).rejects.toMatchObject({
        code: code === "refused" ? "state_connect_failed" : code,
        fatal: true,
      });
      expect([disconnected, f.state.removed]).toEqual([1, true]);
    }

    const f = fakeEnv();
    const promise = createMap(f.env, {} as HTMLElement, { chart: { url: "c.ccf" } });
    f.atlas.reject(new Error("atlas"));
    f.load();
    await expect(promise).resolves.toBeDefined();
  });
});

describe("adapters", () => {
  test("without hold and release the session is manual", async () => {
    const { view } = await loaded();
    expect(view.calls).toContainEqual(["setManualSession", true]);
  });

  test("holds go to the adapter one at a time, in order, and their results back", async () => {
    const order: string[] = [];
    const answers: Array<ReturnType<typeof deferred<HoldResult>>> = [];
    const adapter: StateAdapter = {
      connect: () => Promise.resolve({ kind: "map", unavailable: [] }),
      hold(labels) {
        order.push(`hold ${labels.join()}`);
        const answer = deferred<HoldResult>();
        answers.push(answer);
        return answer.promise;
      },
      release(labels) {
        order.push(`release ${labels.join()}`);
        return Promise.resolve();
      },
      resync: () => Promise.reject(new Error("down")),
      disconnect() {},
    };
    const events: string[] = [];
    const failed: unknown[] = [];
    const errors: string[] = [];
    const { view, drain } = await loaded({
      state: adapter,
      onHoldPending: () => events.push("pending"),
      onHoldSettled: () => events.push("settled"),
      onHoldFailed: (f) => failed.push(f),
      onError: (e) => errors.push(e.code),
    });
    expect(view.names()).not.toContain("setManualSession");
    drain(
      { type: "holdRequest", request: 1, labels: ["A-1-1"] },
      { type: "holdRequest", request: 2, labels: ["A-1-2", "A-1-3"] },
      { type: "releaseRequest", request: 3, labels: ["A-1-1"] },
      { type: "holdRequest", request: 4, labels: ["A-1-1"] },
      { type: "holdRequest", request: 5, labels: ["A-1-2"] },
      { type: "holdRequest", request: 6, labels: ["A-1-3"] },
      { type: "resync", seq: 1 },
    );
    await flush();
    expect(order).toEqual(["hold A-1-1"]);

    answers[0]?.resolve({ ok: true });
    await flush();
    expect(order).toEqual(["hold A-1-1", "hold A-1-2,A-1-3"]);
    answers[1]?.resolve({
      ok: false,
      code: "object_not_free",
      failed: [{ label: "A-1-3", reason: "object_not_free" }],
    });
    await flush();
    expect(order).toEqual(["hold A-1-1", "hold A-1-2,A-1-3", "release A-1-1", "hold A-1-1"]);
    answers[2]?.reject(new Error("offline"));
    await flush();
    // The fourth hold is never answered: it times out. The fifth answers nonsense.
    await sleep(60);
    answers[4]?.resolve(undefined as unknown as HoldResult);
    await flush();

    const results = view.calls.filter((c) => c[0] === "holdOk" || c[0] === "holdFailed");
    expect(results).toEqual([
      ["holdOk", 1],
      ["holdFailed", 2, "object_not_free", ["A-1-3"], ["object_not_free"]],
      ["holdFailed", 4, "hold_failed", [], []],
      ["holdFailed", 5, "hold_failed", [], []],
      ["holdFailed", 6, "hold_failed", [], []],
    ]);
    expect(failed[0]).toEqual([
      { object: expect.objectContaining({ label: "A-1-3" }), reason: "object_not_free" },
    ]);
    expect(failed[1]).toEqual([
      { object: expect.objectContaining({ label: "A-1-1" }), reason: "hold_failed" },
    ]);
    // The rejected async `resync` is reported, not left unhandled.
    expect(errors).toEqual(["state_failed", "hold_failed", "hold_failed", "hold_failed"]);
    expect(events).toEqual(["pending", "settled"]);
  });

  test("label state reaches the view in one call per update; unknown labels are reported", async () => {
    const errors: CaveaError[] = [];
    let ctx: Parameters<StateAdapter["connect"]>[0] | undefined;
    const { view } = await loaded({
      state: customState({
        connect(c) {
          ctx = c;
          return Promise.resolve({
            kind: "map",
            unavailable: ["A-1-1", "X-9"],
            mine: ["A-1-2"],
            expiresAt: 5,
          });
        },
        disconnect() {},
      }),
      onError: (e) => errors.push(e),
    });
    ctx?.applyDelta({ available: ["A-1-1"] });
    ctx?.applySnapshot({ kind: "map", unavailable: [] });
    expect(view.calls.filter((c) => c[0].includes("Labels"))).toEqual([
      ["setSnapshotLabels", 1, ["A-1-1", "X-9"]],
      ["setPrivateLabels", ["A-1-2"], 5],
      ["applyDeltaLabels", 1, 2, [], ["A-1-1"]],
      ["setSnapshotLabels", 3, []],
    ]);
    expect(errors.map((e) => [e.code, e.message])).toEqual([
      ["state_failed", "1 unknown labels, the first X-9"],
    ]);
  });
});

describe("callbacks", () => {
  test("engine events become callbacks with SeatObject payloads", async () => {
    const got: unknown[] = [];
    const log =
      (name: string) =>
      (...args: unknown[]) =>
        got.push([name, ...args]);
    let resynced = 0;
    const { drain } = await loaded({
      state: { ...staticState(), resync: () => (resynced += 1) },
      selection: { min: 1 },
      onSelectionChange: log("change"),
      onObjectSelect: log("select"),
      onObjectDeselect: log("deselect"),
      onSelectionInvalid: log("invalid"),
      onSelectionValid: log("valid"),
      onObjectHover: log("hover"),
      onObjectUnavailable: log("unavailable"),
      onHoldLost: log("lost"),
      onCameraChange: log("camera"),
      onError: (e) => got.push(["error", e.code]),
    });
    const a1 = {
      label: "A-1-1",
      kind: "seat",
      section: "A",
      row: "1",
      seat: "1",
      floor: "Ground",
      category: { key: "vip", label: "VIP" },
      flags: { accessible: true, companion: false, restrictedView: true, aisle: false },
    };
    drain(
      { type: "selectionChanged", expiresAt: null, selected: [seat("A-1-1", 5)] },
      { type: "selectionValid" },
      { type: "selectionChanged", expiresAt: 7, selected: [seat("A-1-2")] },
      { type: "selectionInvalid", rule: "max", objects: ["A-1-3"] },
      { type: "objectHover", object: seat("A-1-1", 5), at: [3, 4] },
      { type: "objectHover", object: null, at: null },
      { type: "objectUnavailable", label: "A-1-2" },
      { type: "holdLost", labels: ["A-1-2"] },
      { type: "cameraChanged", centre: [1, 2], zoom: 0.5, range: [0.1, 10], lod: "seats" },
      { type: "resync", seq: 4 },
      { type: "error", code: "glyphAtlas", message: "404" },
    );
    const a2 = expect.objectContaining({ label: "A-1-2", floor: "Ground" });
    expect(got).toEqual([
      ["select", a1],
      ["change", { objects: [a1], holdToken: null, valid: true, expiresAt: null }],
      ["valid"],
      ["deselect", a1],
      ["select", a2],
      ["change", { objects: [a2], holdToken: null, valid: true, expiresAt: 7 }],
      ["invalid", "max", [expect.objectContaining({ label: "A-1-3" })]],
      ["hover", a1, [3, 4]],
      ["hover", null, null],
      ["unavailable", a2],
      ["lost", [a2]],
      ["camera", { centre: [1, 2], zoom: 0.5, level: "seats" }],
      ["error", "glyph_atlas_failed"],
    ]);
    expect(resynced).toBe(1);
  });

  test("a throwing callback is reported; a throwing onError is not called again", async () => {
    const errors: string[] = [];
    const { drain } = await loaded({
      onSelectionChange: () => {
        throw new Error("host bug");
      },
      onError: (e) => {
        errors.push(e.code);
        throw new Error("worse");
      },
    });
    expect(() => drain({ type: "selectionChanged", expiresAt: null, selected: [] })).not.toThrow();
    expect(errors).toEqual(["callback_failed"]);
  });

  test("events a callback causes are delivered after the batch it is in", async () => {
    const got: unknown[] = [];
    let cleared = false;
    const { map, view, drain } = await loaded({
      selection: { min: 1 },
      onSelectionChange: (s) => {
        got.push(["change", s.objects.length]);
        if (!cleared) {
          cleared = true;
          view.emit(
            { type: "selectionChanged", expiresAt: null, selected: [] },
            { type: "selectionInvalid", rule: "min", objects: [] },
          );
          map.clearSelection();
        }
      },
      onSelectionValid: () => got.push("valid"),
      onSelectionInvalid: (rule) => got.push(["invalid", rule]),
    });
    drain(
      { type: "selectionChanged", expiresAt: null, selected: [seat("A-1-1")] },
      { type: "selectionValid" },
    );
    expect(got).toEqual([["change", 1], "valid", ["change", 0], ["invalid", "min"]]);
  });

  test("a callback that destroys the map ends the batch; no callback or error follows", async () => {
    const got: string[] = [];
    let ctx: Parameters<StateAdapter["connect"]>[0] | undefined;
    const { map, drain } = await loaded({
      state: customState({
        connect(c) {
          ctx = c;
          return Promise.resolve({ kind: "map", unavailable: [] });
        },
        disconnect() {},
      }),
      onSelectionChange: () => {
        got.push("change");
        map.destroy();
      },
      onSelectionValid: () => got.push("valid"),
      onError: (e) => got.push(e.code),
    });
    drain({ type: "selectionChanged", expiresAt: null, selected: [] }, { type: "selectionValid" });
    ctx?.reportError(new Error("late"));
    expect(got).toEqual(["change"]);
  });
});

describe("hold expiry", () => {
  test("onHoldExpiring 60 s before and onHoldExpired with the holds lost, on the server's clock", async () => {
    let ctx: AdapterContext | undefined;
    const calls: unknown[] = [];
    const { map, drain } = await loaded({
      state: customState({
        connect(c) {
          ctx = c;
          return Promise.resolve({ kind: "map", unavailable: [] });
        },
        disconnect() {},
      }),
      onHoldExpiring: (seconds) => calls.push(`expiring ${seconds}`),
      onHoldExpired: () => calls.push("expired"),
      onHoldLost: () => calls.push("lost"),
    });
    // This page's clock is ten minutes ahead of the server's.
    const offset = -600_000;
    ctx?.setClockOffset(offset);
    const expiry = Date.now() + offset + 60_040;
    drain({ type: "selectionChanged", expiresAt: expiry, selected: [seat("A-1-1")] });
    expect(map.getSelection().expiresAt).toBe(expiry - offset);
    await sleep(20);
    expect(calls).toEqual([]);
    await sleep(40);
    // Lost a minute before the expiry (released elsewhere): not an expiry.
    drain({ type: "holdLost", labels: ["A-1-1"] });
    expect(calls).toEqual(["expiring 60", "lost"]);

    drain({
      type: "selectionChanged",
      expiresAt: Date.now() + offset + 2_000,
      selected: [seat("A-1-2")],
    });
    drain(
      { type: "holdLost", labels: ["A-1-2"] },
      { type: "selectionChanged", expiresAt: 0, selected: [] },
    );
    expect(calls).toEqual(["expiring 60", "lost", "expiring 2", "lost", "expired"]);
    expect(map.getSelection().expiresAt).toBeNull();
  });

  test("destroy stops the expiry timer", async () => {
    const calls: unknown[] = [];
    const { map, drain } = await loaded({ onHoldExpiring: () => calls.push("expiring") });
    drain({ type: "selectionChanged", expiresAt: Date.now() + 60_020, selected: [seat("A-1-1")] });
    map.destroy();
    await sleep(40);
    expect(calls).toEqual([]);
  });
});

describe("context and metrics", () => {
  test("a lost context restored in time reports nothing; one that stays lost does", async () => {
    const errors: string[] = [];
    const f = await loaded({ onError: (e) => errors.push(e.code) });
    f.attached()?.onContext?.(false);
    await sleep(10);
    f.attached()?.onContext?.(true);
    await sleep(40);
    expect(errors).toEqual([]);
    f.attached()?.onContext?.(false);
    await sleep(40);
    expect(errors).toEqual(["context_lost"]);
    // Destroyed while lost: the timer goes with the map.
    f.attached()?.onContext?.(false);
    f.map.destroy();
    await sleep(40);
    expect(errors).toEqual(["context_lost"]);
  });

  test("onMetrics: at most once per interval, over the frames drawn, and nothing while idle", async () => {
    const sent: Metrics[] = [];
    const f = await loaded({ onMetrics: (m) => sent.push(m) });
    f.view.answers["frameStats"] = () => Float64Array.of(7, 1200, 3, 2);
    expect(f.attached()?.timeFrames).toBe(true);
    for (const ms of [4, 1, 3, 2, 50]) {
      f.attached()?.onFrame?.(0, ms);
    }
    await sleep(5);
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({
      frames: 5,
      frameMs: { median: 3, p95: 50, max: 50 },
      drawCalls: 7,
      visibleSeats: 1200,
      level: "detail",
      backend: "webgl2",
      pixelRatio: 2,
    });
    expect(sent[0]?.marks["gpu_ready"]).toBeNumber();
    expect(sent[0]?.marks["interactive"]).toBeNumber();
    // Frames within the interval wait for its end; none after, none sent.
    f.attached()?.onFrame?.(0, 6);
    await sleep(5);
    expect(sent).toHaveLength(1);
    await sleep(60);
    expect(sent).toHaveLength(2);
    expect(sent[1]?.frames).toBe(6);
    await sleep(60);
    expect(sent).toHaveLength(2);
  });

  test("without onMetrics frames are not timed", async () => {
    const f = await loaded();
    expect(f.attached()?.timeFrames).toBe(false);
  });
});

describe("methods", () => {
  test("call through to the view and report refused input", async () => {
    const errors: string[] = [];
    let resynced = 0;
    const { map, view, drain, setDark, state } = await loaded({
      state: { ...staticState(), resync: () => (resynced += 1) },
      onError: (e) => errors.push(e.code),
    });
    view.calls.length = 0;
    map.select(["A-1-1"]);
    map.select(["nope"]);
    map.deselect(["A-1-1"]);
    map.clearSelection();
    map.zoomTo("fit");
    map.zoomTo("A", { animate: false });
    map.zoomTo("A-1-2");
    map.zoomTo({ x: 1, y: 2, width: 3, height: 4 });
    drain({ type: "cameraChanged", centre: [100, 50], zoom: 2, range: [0.1, 10], lod: "seats" });
    map.zoomIn();
    setDark(true);
    map.setTheme({ base: "dark", tokens: { bg: "#000", dim: 0.5 } });
    setDark(false);
    expect(state.schemeListeners).toBe(1);
    map.setTheme("sepia" as "dark");
    map.resync();
    expect(view.calls.filter((c) => c[0] !== "select" || (c[1] as string[]).length > 0)).toEqual([
      ["select", ["A-1-1"]],
      ["select", ["nope"]],
      ["deselect", ["A-1-1"]],
      ["clearSelection"],
      ["flyTo", undefined, undefined, false, false],
      ["flyTo", undefined, "A", true, true],
      ["flyTo", undefined, "A-1-2", false, false],
      ["flyTo", [1, 2, 3, 4], undefined, false, false],
      ["flyTo", [6, -19, 188, 138], undefined, false, false],
      ["setTheme", "dark", []],
      ["setTheme", "dark", ["bg", "#000", "dim", "0.5"]],
    ]);
    expect(errors).toEqual(["invalid_input", "invalid_input"]);
    expect(resynced).toBe(1);
    expect(map.findObject("A-1-2")).toMatchObject({ label: "A-1-2", status: "unavailable" });
    expect(map.findObject("Z")).toBeNull();
    expect(map.listSections()).toEqual([{ label: "A", floor: "Ground", seats: 3, available: 2 }]);
    expect(map.listCategories()).toEqual([
      { key: "vip", label: "VIP", color: "#010203ff", objects: 3 },
    ]);
  });

  test("destroy releases everything once; methods after it do nothing", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    let disconnected = 0;
    let signal: AbortSignal | undefined;
    const { map, view, state } = await loaded({
      state: customState({
        connect(ctx) {
          signal = ctx.signal;
          return Promise.resolve({ kind: "map", unavailable: [] });
        },
        disconnect: () => (disconnected += 1),
      }),
    });
    map.destroy();
    map.destroy();
    expect([
      disconnected,
      state.detached,
      state.removed,
      state.schemeListeners,
      signal?.aborted,
    ]).toEqual([1, true, true, 0, true]);
    expect(view.names().filter((n) => n === "free")).toHaveLength(1);
    const before = view.calls.length;
    map.select(["A-1-1"]);
    map.zoomIn();
    map.setTheme("dark");
    expect(map.findObject("A-1-1")).toBeNull();
    expect(map.listSections()).toEqual([]);
    expect(view.calls.length).toBe(before);
    (map as unknown as { setFilter(): void }).setFilter();
    expect(warn.mock.calls.map((c) => c[0])).toEqual([
      "[cavea] the map was destroyed: the call is ignored",
      "[cavea] setFilter() is not supported yet",
    ]);
    warn.mockRestore();
  });
});

test("the package imports without a DOM; createSeatMap then rejects", async () => {
  expect(typeof (globalThis as { document?: unknown }).document).toBe("undefined");
  const sdk = await import("./index.ts");
  await expect(sdk.createSeatMap({} as HTMLElement)).rejects.toMatchObject({ code: "not_browser" });
  // Preloading where there is no DOM does nothing and does not throw.
  await expect(sdk.preloadSeatMap({ chart: { url: "https://x/c.ccf" } })).resolves.toBeUndefined();
});

test("preloadSeatMap fetches a chart once for every later map; a failure is not kept", async () => {
  const { browserEnv, preload } = await import("./browser.ts");
  const g = globalThis as { document?: unknown; fetch: typeof fetch };
  const realFetch = g.fetch;
  const fetched: string[] = [];
  let fail = true;
  g.document = { baseURI: "https://x/" };
  g.fetch = ((url: string | URL) => {
    fetched.push(String(url));
    if (!String(url).endsWith(".ccf")) {
      return Promise.reject(new TypeError("not in this test"));
    }
    return fail
      ? Promise.reject(new TypeError("offline"))
      : Promise.resolve(new Response(new Uint8Array([7])));
  }) as typeof fetch;
  try {
    const charts = (): number => fetched.filter((u) => u.endsWith(".ccf")).length;
    await preload({ chart: { url: "c.ccf" } });
    await preload({ chart: { url: "https://x/c.ccf" } });
    // The first failed and was dropped; the second fetched again.
    expect(charts()).toBe(2);
    fail = false;
    await preload({ chart: { url: "c.ccf" } });
    await preload({ chart: { url: "c.ccf" } });
    expect(charts()).toBe(3);
    // A map takes the preloaded bytes without fetching.
    const bytes = await browserEnv().fetchBytes("https://x/c.ccf", new AbortController().signal);
    expect([[...bytes], charts()]).toEqual([[7], 3]);
  } finally {
    g.fetch = realFetch;
    delete g.document;
  }
});

test("the WASM loads once per page; a failed load is tried again", async () => {
  const { loadWasm } = await import("./browser.ts");
  const { permanent } = await import("./errors.ts");
  // An HTTP failure keeps its status, so a 404 is not retried (05 §9).
  const g = globalThis as { fetch: typeof fetch };
  const realFetch = g.fetch;
  g.fetch = (() => Promise.resolve(new Response("", { status: 404 }))) as unknown as typeof fetch;
  try {
    const failure = await loadWasm({ wasm: "https://x/cavea_web_bg.wasm" }).catch(
      (e: unknown) => e,
    );
    expect([(failure as { status?: number }).status, permanent(failure)]).toEqual([404, true]);
  } finally {
    g.fetch = realFetch;
  }
  await expect(loadWasm({ wasm: new Uint8Array([0, 1, 2, 3]) })).rejects.toThrow();
  const bytes = await Bun.file(new URL("../wasm/cavea_web_bg.wasm", import.meta.url)).arrayBuffer();
  const first = loadWasm({ wasm: bytes });
  expect(loadWasm()).toBe(first);
  await first;
  const { version } = await import("../wasm/cavea_web.js");
  expect(version()).toMatch(/^\d+\.\d+\.\d+/);
});
