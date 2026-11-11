// `createSeatMap` (docs/05-web-sdk.md §3–§6) over an `Env`: the browser's in
// `browser.ts`, a fake in tests. The load sequence is 04 §4: the WASM, chart,
// state and atlas start together; the first frame waits only for the WASM
// and the chart; state that arrives first is applied once the chart is in.
import {
  staticState,
  type AdapterContext,
  type ChartSource,
  type HoldResult,
  type Snapshot,
} from "./adapter.ts";
import { CaveaError, isDevelopment, warner, type CaveaErrorCode } from "./errors.ts";
import type { ChartInfo, ObjectInfo, RendererEvent } from "./events.ts";
import type { AttachOptions, Attached } from "./host.ts";
import {
  isBounds,
  isTheme,
  validateOptions,
  type Bounds,
  type SeatMapOptions,
  type SeatObject,
  type Selection,
  type Theme,
} from "./options.ts";
import type { EngineView } from "./view.ts";

/** What `createSeatMap` needs from its platform. */
export interface Env {
  /** Starts loading the WASM, shared by every map on the page; resolves with a view factory. */
  wasm(assets: SeatMapOptions["assets"]): Promise<(canvas: HTMLCanvasElement) => EngineView>;
  fetchBytes(url: string | URL, signal: AbortSignal): Promise<Uint8Array>;
  /** The glyph atlas's pixels. */
  atlas(assets: SeatMapOptions["assets"]): Promise<Uint8Array>;
  /** A canvas in a shadow root of `element`. */
  mount(element: HTMLElement): {
    canvas: HTMLCanvasElement;
    size(): [number, number];
    remove(): void;
  };
  attach(view: EngineView, canvas: HTMLCanvasElement, options: AttachOptions): Attached;
  /** Calls `listener` with whether the system prefers dark, now and on change; returns the unsubscribe. */
  darkScheme(listener: (dark: boolean) => void): () => void;
  /** After this long without an answer, a hold or release counts as failed. */
  holdTimeoutMs: number;
}

export type ObjectStatus = "available" | "unavailable" | "selected";

/** The map `createSeatMap` resolves with (05 §6). Methods never throw; after `destroy` they do nothing. */
export interface SeatMap {
  getSelection(): Selection;
  /** As taps, all or nothing; unknown labels are reported through `onError`. */
  select(labels: string[]): void;
  deselect(labels: string[]): void;
  clearSelection(): void;
  setTheme(theme: Theme): void;
  /** `fit`, a section label, a seat label, or bounds. Animated unless `animate` is false. */
  zoomTo(target: "fit" | string | Bounds, options?: { animate?: boolean }): void;
  zoomIn(): void;
  zoomOut(): void;
  findObject(label: string): (SeatObject & { status: ObjectStatus }) | null;
  /** `available`: seats not drawn unavailable (free, or selected by this buyer). */
  listSections(): Array<{ label: string; floor: string; seats: number; available: number }>;
  /** `color`: `#rrggbbaa`; `objects`: bookable objects in the category. No availability counts yet. */
  listCategories(): Array<{ key: string; label: string; color: string; objects: number }>;
  /** Asks the adapter for a fresh snapshot. */
  resync(): void;
  /** Releases the GL context, the adapter's connection and listeners. Does not release holds. */
  destroy(): void;
}

/** 05 §6 methods whose feature is not built: they warn in development and do nothing. */
const LATER_METHODS =
  "setPricing setFilter setLocale setMode setSpotlight setFloor selectBestAvailable showListView toImage";

type Callbacks = {
  [K in keyof SeatMapOptions as K extends `on${string}` ? K : never]-?: NonNullable<
    SeatMapOptions[K]
  >;
};

/** Starts `promise`'s work now; a synchronous throw becomes a rejection. */
function attempt<T>(start: () => T | Promise<T>): Promise<T> {
  try {
    return Promise.resolve(start());
  } catch (error) {
    return Promise.reject(error);
  }
}

/** `error` as a `CaveaError`, keeping one that already is. */
const failure = (code: CaveaErrorCode, error: unknown, fatal = false): CaveaError =>
  error instanceof CaveaError ? error : new CaveaError(code, String(error), fatal, error);

/** `promise`, rejecting with a fatal `CaveaError` of `code` whose cause is the original error. */
const fatal = <T>(promise: Promise<T>, code: CaveaErrorCode): Promise<T> =>
  promise.catch((error: unknown) => {
    throw new CaveaError(code, String(error), true, error);
  });

/** `onHoldExpiring` fires this long before the buyer's holds expire (05 §5). */
const EXPIRING_MS = 60_000;
/** Holds lost this close to their expiry, or after it, expired (`onHoldExpired`). */
const EXPIRED_SLACK_MS = 5_000;

const bytesOf = (b: ArrayBuffer | Uint8Array): Uint8Array =>
  b instanceof Uint8Array ? b : new Uint8Array(b);

export async function createMap(
  env: Env,
  element: HTMLElement,
  input?: SeatMapOptions,
): Promise<SeatMap> {
  const start = performance.now();
  const marks: Record<string, number> = {};
  const mark = (name: string): void => {
    marks[name] = performance.now() - start;
    performance.mark(`cavea:${name}`);
  };
  const warn = warner(isDevelopment());
  const options = validateOptions(input, warn);
  mark("init_start");
  const adapter = options.state ?? staticState();
  const abort = new AbortController();
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const max = options.selection?.max ?? 10;
  const min = options.selection?.min ?? 0;

  let view: EngineView | undefined;
  let host: Attached | undefined;
  let destroyed = false;
  let chart: ChartInfo | undefined;
  /** Adapter calls made before the first snapshot was applied, in order. */
  let pending: Array<(view: EngineView) => void> | undefined = [];
  /** The sequence of label state, which the SDK numbers itself. */
  let seq = 0;
  let holdToken: string | null = null;
  let selected = new Map<string, SeatObject>();
  /** When the buyer's holds expire, on the server's clock (Unix ms). */
  let expiresAt: number | null = null;
  /** The server's clock minus this page's, from the adapter. */
  let offset = 0;
  /** The expiry `onHoldExpiring` and `onHoldExpired` watch (server ms), and whether the first fired for it. */
  let deadline: number | null = null;
  let warned = false;
  let expiryTimer: ReturnType<typeof setTimeout> | undefined;
  let camera: { centre: [number, number]; zoom: number } | undefined;
  let unwatchTheme: (() => void) | undefined;
  let outstanding = 0;
  let queue: Promise<void> = Promise.resolve();

  /** Tells the host; an `onError` that throws is not called again for its own throw. */
  function report(error: CaveaError): void {
    if (options.onError === undefined || destroyed) {
      warn(`${error.code}: ${error.message}`);
      return;
    }
    try {
      options.onError(error);
    } catch {
      // Nothing is left to tell.
    }
  }

  function call<K extends keyof Callbacks>(name: K, ...args: Parameters<Callbacks[K]>): void {
    const callback = options[name] as ((...a: Parameters<Callbacks[K]>) => void) | undefined;
    if (callback === undefined || destroyed) {
      return;
    }
    try {
      callback(...args);
    } catch (error) {
      report(new CaveaError("callback_failed", `${name} threw: ${String(error)}`, false, error));
    }
  }

  /**
   * Runs an engine or adapter call: a throw or a rejected promise is
   * reported, not thrown; unknown labels the view returns are reported.
   */
  function engine(
    run: () => unknown,
    code: CaveaErrorCode = "engine_failed",
    strict = false,
  ): void {
    try {
      const unknown = run();
      if (unknown instanceof Promise) {
        unknown.catch((error: unknown) => report(failure(code, error)));
      } else if (Array.isArray(unknown) && unknown.length > 0) {
        report(
          new CaveaError(
            "state_failed",
            `${unknown.length} unknown labels, the first ${unknown[0]}`,
          ),
        );
      }
    } catch (error) {
      if (strict) {
        throw new CaveaError(code, String(error), true, error);
      }
      report(failure(code, error));
    }
  }

  /** Runs `apply` on the view once the map is ready, in order with earlier calls. */
  function later(apply: (view: EngineView) => void): void {
    if (destroyed) {
      return;
    }
    if (pending !== undefined || view === undefined) {
      pending?.push(apply);
      return;
    }
    apply(view);
    host?.update();
  }

  /** `first`: the state `connect` resolved with, which the map cannot start without. */
  function applySnapshot(v: EngineView, snapshot: Snapshot, first = false): void {
    const code = first ? "state_connect_failed" : "state_failed";
    if (snapshot.kind === "binary") {
      engine(() => v.applyFrame(bytesOf(snapshot.bytes)), code, first);
      return;
    }
    seq += 1;
    engine(() => v.setSnapshotLabels(seq, snapshot.unavailable), code, first);
    if (snapshot.mine !== undefined) {
      const mine = snapshot.mine;
      engine(() => v.setPrivateLabels(mine, snapshot.expiresAt ?? 0), code, first);
    }
  }

  const ctx: AdapterContext = {
    signal: abort.signal,
    applyFrame: (bytes) => later((v) => engine(() => v.applyFrame(bytesOf(bytes)), "state_failed")),
    applySnapshot: (snapshot) => later((v) => applySnapshot(v, snapshot)),
    applyDelta: (delta) =>
      later((v) => {
        const from = seq;
        seq += 1;
        engine(
          () => v.applyDeltaLabels(from, seq, delta.unavailable ?? [], delta.available ?? []),
          "state_failed",
        );
      }),
    setMine: (labels, expiry) =>
      later((v) => engine(() => v.setPrivateLabels(labels, expiry ?? 0), "state_failed")),
    setHoldToken: (token) => {
      holdToken = token;
    },
    setClockOffset: (ms) => {
      if (Number.isFinite(ms)) {
        offset = ms;
        watchExpiry();
      }
    },
    reportError: (error) => report(failure("state_failed", error)),
  };

  function toSeat(o: ObjectInfo): SeatObject {
    const section = chart?.sections.find((s) => s.label === o.section && o.section !== "");
    const category = chart?.categories.find((c) => c.key === o.category);
    const seat: SeatObject = {
      label: o.label,
      kind: "seat",
      floor: chart?.floors[section?.floor ?? 0]?.name ?? "",
      category: { key: o.category, label: category?.label ?? o.category },
      flags: {
        accessible: (o.flags & 1) !== 0,
        companion: (o.flags & 2) !== 0,
        restrictedView: (o.flags & 4) !== 0,
        aisle: (o.flags & 8) !== 0,
      },
    };
    for (const key of ["section", "row", "seat"] as const) {
      if (o[key] !== "") {
        seat[key] = o[key];
      }
    }
    return seat;
  }

  /** An object's details and public status code, from the view. */
  function details(label: string): (ObjectInfo & { status: number }) | undefined {
    const json = view?.object(label);
    return json === undefined ? undefined : (JSON.parse(json) as ObjectInfo & { status: number });
  }

  function lookup(label: string): SeatObject {
    const info = details(label) ?? {
      label,
      category: "",
      section: "",
      row: "",
      seat: "",
      flags: 0,
    };
    return selected.get(label) ?? toSeat(info);
  }

  const selection = (): Selection => ({
    objects: [...selected.values()],
    holdToken,
    valid: selected.size >= min,
    expiresAt: expiresAt === null ? null : expiresAt - offset,
  });

  const serverNow = (): number => Date.now() + offset;

  /**
   * Fires `onHoldExpiring` once per expiry, `EXPIRING_MS` before it (at once,
   * with the seconds left, if it is nearer). One timer, recomputed when it
   * fires and when the page is shown again, as hidden pages run timers late.
   */
  function watchExpiry(): void {
    clearTimeout(expiryTimer);
    timers.delete(expiryTimer as ReturnType<typeof setTimeout>);
    const left = deadline === null ? 0 : deadline - serverNow();
    if (warned || destroyed || left <= 0) {
      return;
    }
    if (left <= EXPIRING_MS) {
      warned = true;
      call("onHoldExpiring", Math.ceil(left / 1000));
      return;
    }
    expiryTimer = setTimeout(watchExpiry, left - EXPIRING_MS);
    timers.add(expiryTimer);
  }

  /** Runs adapter calls one at a time, in order; each fails after `holdTimeoutMs`. */
  function enqueue(task: () => Promise<void>): void {
    queue = queue
      .then(() => (destroyed ? undefined : task()))
      .catch((error: unknown) => report(failure("state_failed", error)));
  }
  function timed<T>(run: () => Promise<T> | undefined): Promise<T> {
    // Set by the executor, which runs before `finally` can.
    let timer!: ReturnType<typeof setTimeout>;
    return new Promise<T>((resolve, reject) => {
      timer = setTimeout(
        () => reject(new Error(`no answer in ${env.holdTimeoutMs} ms`)),
        env.holdTimeoutMs,
      );
      timers.add(timer);
      attempt(() => run() ?? Promise.reject(new Error("the adapter has no hold or release"))).then(
        resolve,
        reject,
      );
    }).finally(() => {
      clearTimeout(timer);
      timers.delete(timer);
    });
  }

  function hold(request: number, labels: string[]): void {
    if (outstanding++ === 0) {
      call("onHoldPending");
    }
    enqueue(async () => {
      let result: HoldResult;
      try {
        result = await timed(() => adapter.hold?.(labels));
        // The engine waits for an answer to every hold: a malformed one is a failure.
        if (typeof result?.ok !== "boolean") {
          throw new Error(`hold() resolved with ${String(result)}, not a HoldResult`);
        }
      } catch (error) {
        report(new CaveaError("hold_failed", String(error), false, error));
        result = { ok: false, code: "hold_failed" };
      }
      if (!destroyed && view !== undefined) {
        const v = view;
        if (result.ok) {
          engine(() => v.holdOk(request));
        } else {
          const code = result.code ?? result.failed?.[0]?.reason ?? "hold_failed";
          const named = (Array.isArray(result.failed) ? result.failed : [])
            .filter((f) => labels.includes(f?.label))
            .map((f) => ({ label: f.label, reason: String(f.reason ?? code) }));
          const failed = (
            named.length > 0 ? named : labels.map((label) => ({ label, reason: code }))
          ).map((f) => ({ object: lookup(f.label), reason: f.reason }));
          engine(() =>
            v.holdFailed(
              request,
              code,
              named.map((f) => f.label),
              named.map((f) => f.reason),
            ),
          );
          // The engine's reverts first, then the reasons.
          host?.update();
          call("onHoldFailed", failed);
        }
        host?.update();
      }
      outstanding -= 1;
      if (outstanding === 0) {
        call("onHoldSettled");
      }
    });
  }

  /** Events still to deliver; set while a batch is being delivered. */
  let delivering: RendererEvent[] | undefined;

  /**
   * Delivers events in order. A method called from a callback drains newer
   * events; they wait until the batch being delivered is done.
   */
  function onEvents(batch: RendererEvent[]): void {
    if (delivering !== undefined) {
      delivering.push(...batch);
      return;
    }
    delivering = [...batch];
    try {
      for (const event of delivering) {
        // A callback may destroy the map.
        if (destroyed) {
          break;
        }
        deliver(event);
      }
    } finally {
      delivering = undefined;
    }
  }

  function deliver(event: RendererEvent): void {
    switch (event.type) {
      case "ready":
        chart = event;
        break;
      case "selectionChanged": {
        const before = selected;
        selected = new Map(event.selected.map((o) => [o.label, toSeat(o)]));
        // A set of own holds without an expiry reports 0.
        expiresAt = event.expiresAt || null;
        if (expiresAt !== null && expiresAt !== deadline) {
          deadline = expiresAt;
          warned = false;
        } else if (expiresAt === null && serverNow() < (deadline ?? 0) - EXPIRED_SLACK_MS) {
          // Given up before the expiry: nothing is left to expire.
          deadline = null;
        }
        watchExpiry();
        for (const [label, object] of before) {
          if (!selected.has(label)) {
            call("onObjectDeselect", object);
          }
        }
        for (const [label, object] of selected) {
          if (!before.has(label)) {
            call("onObjectSelect", object);
          }
        }
        call("onSelectionChange", selection());
        break;
      }
      case "selectionInvalid":
        call("onSelectionInvalid", event.rule, event.objects.map(lookup));
        break;
      case "selectionValid":
        call("onSelectionValid");
        break;
      case "holdRequest":
        hold(event.request, event.labels);
        break;
      case "releaseRequest": {
        const labels = event.labels;
        enqueue(() =>
          timed(() => adapter.release?.(labels)).catch((error: unknown) =>
            report(new CaveaError("release_failed", String(error), false, error)),
          ),
        );
        break;
      }
      case "objectHover":
        call("onObjectHover", event.object && toSeat(event.object), event.at);
        break;
      case "objectFocus":
        call("onObjectFocus", event.object && toSeat(event.object), event.at);
        break;
      case "objectUnavailable":
        call("onObjectUnavailable", lookup(event.label));
        break;
      case "holdLost":
        call("onHoldLost", event.labels.map(lookup));
        if (deadline !== null && serverNow() >= deadline - EXPIRED_SLACK_MS) {
          deadline = null;
          call("onHoldExpired");
        }
        break;
      case "resync":
        if (adapter.resync === undefined) {
          report(
            new CaveaError("state_failed", "state is out of step and the adapter cannot resync"),
          );
        } else {
          engine(() => adapter.resync?.(), "state_failed");
        }
        break;
      case "cameraChanged":
        camera = { centre: event.centre, zoom: event.zoom };
        call("onCameraChange", { centre: event.centre, zoom: event.zoom, level: event.lod });
        break;
      case "error":
        report(
          new CaveaError(
            event.code === "glyphAtlas" ? "glyph_atlas_failed" : "engine_failed",
            event.message,
          ),
        );
        break;
    }
  }

  function applyTheme(theme: Theme): void {
    unwatchTheme?.();
    unwatchTheme = undefined;
    const base = typeof theme === "string" ? theme : (theme.base ?? "light");
    const tokens =
      typeof theme === "string"
        ? []
        : Object.entries(theme.tokens).flatMap(([name, value]) => [name, String(value)]);
    const apply = (name: string): void => {
      engine(() => view?.setTheme(name, tokens), "invalid_input");
      host?.update();
    };
    if (base === "auto") {
      unwatchTheme = env.darkScheme((dark) => apply(dark ? "dark" : "light"));
    } else {
      apply(base);
    }
  }

  function fly(
    rect: Bounds | undefined,
    label: string | undefined,
    section: boolean,
    animate: boolean,
  ): void {
    const r = rect && Float32Array.of(rect.x, rect.y, rect.width, rect.height);
    engine(() => view?.flyTo(r, label, section, !animate), "invalid_input");
    host?.update();
  }
  /** Moves to `label`: a section on the floor shown if one has it, else a seat. */
  const flyToLabel = (label: string, animate: boolean): void =>
    fly(
      undefined,
      label,
      chart?.sections.some((s) => s.label === label && s.floor === 0) ?? false,
      animate,
    );

  function teardown(): void {
    destroyed = true;
    for (const timer of timers) {
      clearTimeout(timer);
    }
    timers.clear();
    abort.abort();
    unwatchTheme?.();
    engine(() => adapter.disconnect(), "state_failed");
    host?.detach();
    view?.free();
    view = undefined;
    mounted?.remove();
  }

  // The four fetches start here, together (04 §4).
  const wasm = attempt(() => env.wasm(options.assets));
  const load = (source: ChartSource): Promise<Uint8Array> =>
    "bytes" in source
      ? Promise.resolve(bytesOf(source.bytes))
      : env.fetchBytes(source.url, abort.signal);
  const chartBytes = attempt(() => {
    if (options.chart !== undefined) {
      return load(options.chart);
    }
    if (adapter.chart === undefined) {
      throw new Error("no chart: pass options.chart or an adapter with chart()");
    }
    return adapter.chart(abort.signal).then(load);
  });
  const state = attempt(() => adapter.connect(ctx));
  const atlas = attempt(() => env.atlas(options.assets));
  // Each is awaited or handed over below; none is an unhandled rejection meanwhile.
  for (const promise of [wasm, chartBytes, state, atlas]) {
    promise.catch(() => undefined);
  }
  // A failed chart or connect ends the start at once, whatever is still loading.
  const failed = new Promise<never>((_, reject) => {
    fatal(chartBytes, "chart_fetch_failed").catch(reject);
    fatal(state, "state_connect_failed").catch(reject);
  });
  failed.catch(() => undefined);
  const step = <T>(promise: Promise<T>): Promise<T> => Promise.race([promise, failed]);
  let mounted: ReturnType<Env["mount"]> | undefined;
  try {
    mounted = env.mount(element);
    const canvas = mounted.canvas;
    const makeView = await step(fatal(wasm, "wasm_unavailable"));
    mark("wasm_ready");
    const v = await fatal(
      attempt(() => makeView(canvas)),
      "webgl_unavailable",
    );
    view = v;
    v.setRules(max, min);
    if (adapter.hold === undefined) {
      v.setManualSession(true);
    }
    const attachOptions: AttachOptions = {
      gestureHandling: options.gestureHandling ?? "greedy",
      glyphAtlas: atlas,
      onEvents,
      onFrame: () => {
        if (chart !== undefined && marks["first_frame"] === undefined) {
          mark("first_frame");
        }
      },
      onVisibility: (visible) => {
        engine(() => (visible ? adapter.resume?.() : adapter.pause?.()), "state_failed");
        if (visible) {
          watchExpiry();
        }
      },
    };
    if (options.maxDpr !== undefined) {
      attachOptions.maxDpr = options.maxDpr;
    }
    if (typeof options.reducedMotion === "boolean") {
      attachOptions.reducedMotion = options.reducedMotion;
    }
    host = env.attach(v, canvas, attachOptions);
    applyTheme(options.theme ?? "auto");

    const bytes = await step(fatal(chartBytes, "chart_fetch_failed"));
    mark("chart_fetched");
    try {
      v.loadChart(bytes);
    } catch (error) {
      throw new CaveaError("bad_chart", String(error), true, error);
    }
    host.update();
    mark("chart_decoded");
    const initial = options.initialView;
    if (isBounds(initial)) {
      fly(initial, undefined, false, false);
    } else if (initial !== undefined && initial !== "fit") {
      flyToLabel(initial, false);
    }

    const first = await step(fatal(state, "state_connect_failed"));
    applySnapshot(v, first, true);
    for (const apply of pending ?? []) {
      apply(v);
    }
    pending = undefined;
    host.update();
    mark("state_ready");
  } catch (error) {
    teardown();
    throw failure("engine_failed", error, true);
  }

  /** The view while the map lives; after `destroy`, a development warning. */
  const live = (): EngineView | undefined => {
    if (view === undefined) {
      warn("the map was destroyed: the call is ignored");
    }
    return view;
  };
  const labelsCall = (run: (v: EngineView) => void): void => {
    const v = live();
    if (v !== undefined) {
      engine(() => run(v), "invalid_input");
      host?.update();
    }
  };

  const map: SeatMap = {
    getSelection: selection,
    select: (labels) => labelsCall((v) => v.select(labels)),
    deselect: (labels) => labelsCall((v) => v.deselect(labels)),
    clearSelection: () => labelsCall((v) => v.clearSelection()),
    setTheme(theme) {
      if (live() === undefined) {
        return;
      }
      if (isTheme(theme)) {
        applyTheme(theme);
      } else {
        report(new CaveaError("invalid_input", `invalid theme: ${String(theme)}`));
      }
    },
    zoomTo(target, { animate = true } = {}) {
      if (live() === undefined) {
        return;
      }
      if (target === "fit") {
        fly(undefined, undefined, false, animate);
      } else if (typeof target === "string") {
        flyToLabel(target, animate);
      } else if (isBounds(target)) {
        fly(target, undefined, false, animate);
      } else {
        report(new CaveaError("invalid_input", `invalid zoom target: ${String(target)}`));
      }
    },
    zoomIn: () => zoomBy(2),
    zoomOut: () => zoomBy(0.5),
    findObject(label) {
      const info = live() && details(label);
      if (info === undefined) {
        return null;
      }
      const status: ObjectStatus = selected.has(label)
        ? "selected"
        : info.status === 0
          ? "available"
          : "unavailable";
      return { ...toSeat(info), status };
    },
    listSections() {
      const open = live()?.sectionsOpen();
      if (open === undefined) {
        return [];
      }
      return (chart?.sections ?? []).map((s, i) => ({
        label: s.label,
        floor: chart?.floors[s.floor]?.name ?? "",
        seats: s.seats,
        available: open[i] ?? 0,
      }));
    },
    listCategories: () =>
      (live() &&
        chart?.categories.map((c) => ({
          ...c,
          color: `#${c.color.map((v) => v.toString(16).padStart(2, "0")).join("")}`,
        }))) ??
      [],
    resync() {
      if (live() === undefined) {
        return;
      }
      if (adapter.resync === undefined) {
        warn("the state adapter has no resync()");
      } else {
        engine(() => adapter.resync?.(), "state_failed");
      }
    },
    destroy() {
      if (!destroyed) {
        teardown();
      }
    },
  };
  function zoomBy(factor: number): void {
    const size = mounted?.size();
    if (live() === undefined || camera === undefined || size === undefined) {
      return;
    }
    const scale = camera.zoom * factor;
    // The engine fits bounds inside a margin of `FIT_PADDING_PX` (24) on each side.
    const room = (px: number): number => Math.max(px - 48, 1) / scale;
    const [width, height] = [room(size[0]), room(size[1])];
    const [x, y] = camera.centre;
    fly({ x: x - width / 2, y: y - height / 2, width, height }, undefined, false, true);
  }
  for (const name of LATER_METHODS.split(" ")) {
    (map as unknown as Record<string, () => void>)[name] = () =>
      warn(`${name}() is not supported yet`);
  }

  call("onReady", {
    chart: { name: chart?.name ?? "", seats: chart?.seats ?? 0, objects: chart?.objects ?? 0 },
    backend: "webgl2",
    marks: { ...marks },
  });
  return map;
}
