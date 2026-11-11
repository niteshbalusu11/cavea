// Fakes of the WASM view and the platform for the SDK's tests: no browser,
// no WebGL. The DOM is `fakedom.ts`'s.
import type { ObjectInfo, RendererEvent } from "./events.ts";
import { FakeDocument } from "./fakedom.ts";
import type { AttachOptions } from "./host.ts";
import type { SeatMapOptions } from "./options.ts";
import { createMap, type Env } from "./seatmap.ts";
import type { EngineView } from "./view.ts";

type Call = [string, ...unknown[]];

export function deferred<T>(): {
  promise: Promise<T>;
  resolve(v: T): void;
  reject(e: unknown): void;
} {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Lets every pending promise reaction run. */
export const flush = (): Promise<void> => new Promise((r) => setTimeout(r, 0));
export const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

export const READY: RendererEvent = {
  type: "ready",
  name: "Hall",
  floors: [{ name: "Ground", level: 0, seats: 3 }],
  sections: [{ label: "A", floor: 0, seats: 3 }],
  categories: [{ key: "vip", label: "VIP", color: [1, 2, 3, 255], objects: 3 }],
  objects: 3,
  seats: 3,
};
export const seat = (label: string, flags = 0): ObjectInfo => ({
  label,
  category: "vip",
  section: "A",
  row: "1",
  seat: label.slice(-1),
  flags,
});

/** A recording fake of the view: every call is logged, a few answer as the engine does. */
export function fakeView() {
  const calls: Call[] = [];
  const queue: RendererEvent[] = [];
  const answers: Record<string, (...args: never[]) => unknown> = {
    loadChart: () => queue.push(READY),
    setSnapshotLabels: (_seq: number, labels: string[]) => labels.filter((l) => l.startsWith("X")),
    select: (labels: string[]) => {
      if (labels.includes("nope")) {
        throw new Error("InvalidInput: 1 unknown objects");
      }
    },
    sectionsOpen: () => Uint32Array.of(2),
    object: (label: string) =>
      label.startsWith("A-") ? JSON.stringify({ ...seat(label), status: 1 }) : undefined,
    takeEvents: () => {
      const events = queue.splice(0);
      return events.length === 0 ? undefined : JSON.stringify(events);
    },
  };
  const engine = new Proxy({} as EngineView, {
    // Not a thenable: `then` stays undefined.
    get: (_, name: string) =>
      name === "then" || name === "needFrame"
        ? undefined
        : (...args: never[]) => {
            if (name !== "takeEvents" && name !== "object") {
              calls.push([
                name,
                ...args.map((a: unknown) => (a instanceof Float32Array ? [...a] : a)),
              ]);
            }
            return answers[name]?.(...args) ?? (name.endsWith("Labels") ? [] : undefined);
          },
  });
  return {
    engine,
    answers,
    calls,
    emit: (...events: RendererEvent[]) => queue.push(...events),
    names: () => calls.map((c) => c[0]),
  };
}

/** A platform whose fetches the test settles. */
export function fakeEnv() {
  const view = fakeView();
  const wasm = deferred<(canvas: HTMLCanvasElement) => EngineView>();
  const chart = deferred<Uint8Array>();
  const atlas = deferred<Uint8Array>();
  const started: string[] = [];
  const doc = new FakeDocument();
  const root = doc.createElement("div");
  const canvas = doc.createElement("canvas");
  root.append(canvas);
  doc.body.append(root);
  const state = {
    removed: false,
    detached: false,
    schemeListeners: 0,
    watchers: new Set<() => void>(),
    /** `--cavea-*` custom properties the page sets, as computed on the root (without the prefix). */
    css: {} as Record<string, string>,
    makeView: (): EngineView => view.engine,
    /** `navigator.onLine`, and whether the page is shown. */
    online: true,
    shown: true,
    /** Waiters of `usable`, released by `setUsable`. */
    waiting: [] as Array<() => void>,
  };
  let attached: AttachOptions | undefined;
  let dark: ((dark: boolean) => void) | undefined;
  const env: Env = {
    wasm: () => {
      started.push("wasm");
      return wasm.promise;
    },
    fetchBytes: () => {
      started.push("chart");
      return chart.promise;
    },
    atlas: () => {
      started.push("atlas");
      return atlas.promise;
    },
    mount: () => ({
      canvas: canvas as unknown as HTMLCanvasElement,
      root: root as unknown as HTMLElement,
      size: () => [800, 600],
      // The cascade: the root's own (inline) tokens over the page's.
      tokens: (names) =>
        names.map((n) => root.style.getPropertyValue(`--cavea-${n}`) || (state.css[n] ?? "")),
      watch(listener) {
        state.watchers.add(listener);
        return () => state.watchers.delete(listener);
      },
      remove: () => {
        state.removed = true;
        root.remove();
      },
    }),
    attach: (v, _canvas, options) => {
      attached = options;
      const update = (): void => {
        const json = v.takeEvents();
        if (json !== undefined) {
          options.onEvents?.(JSON.parse(json) as RendererEvent[]);
        }
      };
      return {
        update,
        setGestureHandling() {},
        detach: () => {
          state.detached = true;
        },
      };
    },
    darkScheme(listener) {
      state.schemeListeners += 1;
      dark = listener;
      listener(false);
      return () => {
        state.schemeListeners -= 1;
        dark = undefined;
      };
    },
    online: () => state.online,
    usable: (signal) =>
      state.online && state.shown
        ? Promise.resolve()
        : new Promise((resolve) => {
            state.waiting.push(resolve);
            signal.addEventListener("abort", () => state.waiting.splice(0));
          }),
    retryMs: 1,
    holdTimeoutMs: 30,
    contextLostMs: 30,
    metricsMs: 40,
  };
  return {
    env,
    view,
    wasm,
    chart,
    atlas,
    started,
    state,
    attached: () => attached,
    setDark: (on: boolean) => dark?.(on),
    doc,
    root,
    canvas,
    /** The page restyled: every watcher hears of it. */
    restyle(css: Record<string, string>): void {
      state.css = css;
      for (const listener of state.watchers) {
        listener();
      }
    },
    /** The page goes on- or offline, or is hidden or shown. */
    setUsable(online: boolean, shown = true): void {
      state.online = online;
      state.shown = shown;
      if (online && shown) {
        for (const resume of state.waiting.splice(0)) {
          resume();
        }
      }
    },
    /** Settles WASM and chart (the view comes from `state.makeView`). */
    load(): void {
      wasm.resolve(() => state.makeView());
      chart.resolve(new Uint8Array([1]));
    },
  };
}

/** A map loaded with `adapter` and `options`, every fetch settled; `view.emit` then `drain` drives it. */
export async function loaded(options: SeatMapOptions = {}) {
  const f = fakeEnv();
  const promise = createMap(f.env, {} as HTMLElement, { chart: { url: "c.ccf" }, ...options });
  f.load();
  const map = await promise;
  const drain = (...events: RendererEvent[]): void => {
    f.view.emit(...events);
    // Any method's `host.update()` drains; `select([])` is one the fake accepts.
    map.select([]);
  };
  return { ...f, map, drain };
}
