// `createSeatMap` (docs/05-web-sdk.md §3–§6) over an `Env`: the browser's in
// `browser.ts`, a fake in tests. The load sequence is 04 §4: the WASM, chart,
// state and atlas start together; the first frame waits only for the WASM
// and the chart; state that arrives first is applied once the chart is in.
import { staticState, type AdapterContext, type HoldResult, type Snapshot } from "./adapter.ts";
import { createA11y, type A11y } from "./a11y.ts";
import { CaveaError, isDevelopment, permanent, warner, type CaveaErrorCode } from "./errors.ts";
import type { ChartInfo, ObjectInfo, RendererEvent } from "./events.ts";
import type { ObjectDetails } from "./generated/engine.ts";
import type { TrustedKey } from "./generated/licence-keys.ts";
import type { AttachOptions, Attached } from "./host.ts";
import { check, decide, reported, type Standing } from "./licence.ts";
import { localeDirection, translator } from "./messages.ts";
import {
  isBounds,
  isLocale,
  isTheme,
  validateOptions,
  type Bounds,
  type Metrics,
  type SeatMapOptions,
  type SeatObject,
  type Selection,
  type Theme,
  type ThemeName,
} from "./options.ts";
import { checkPricing, prices, type CategoryPricing } from "./pricing.ts";
import { CANVAS_TOKENS } from "./theme.ts";
import { createUi, type TooltipObject, type UiHost } from "./ui.ts";
import type { EngineView } from "./view.ts";

/** What `createSeatMap` needs from its platform. */
export interface Env {
  /** Starts loading the WASM, shared by every map on the page; resolves with a view factory. */
  wasm(assets: SeatMapOptions["assets"]): Promise<(canvas: HTMLCanvasElement) => EngineView>;
  fetchBytes(url: string | URL, signal: AbortSignal): Promise<Uint8Array>;
  /** The glyph atlas's pixels. */
  atlas(assets: SeatMapOptions["assets"]): Promise<Uint8Array>;
  /** A canvas, and the interface's root over it, in a shadow root of `element`. */
  mount(element: HTMLElement): Mounted;
  attach(view: EngineView, canvas: HTMLCanvasElement, options: AttachOptions): Attached;
  /** Calls `listener` with whether the system prefers dark, now and on change; returns the unsubscribe. */
  darkScheme(listener: (dark: boolean) => void): () => void;
  /** After this long without an answer, a hold or release counts as failed. */
  holdTimeoutMs: number;
  /** Whether the browser says it is online. */
  online(): boolean;
  /** Resolves when the page is shown and online: at once if it is; never after `signal` aborts. */
  usable(signal: AbortSignal): Promise<void>;
  /** The first retry's delay before jitter; each next doubles (05 §9: 1 s). */
  retryMs: number;
  /** A lost context not restored within this long is reported (05 §9: 5 s). */
  contextLostMs: number;
  /** `onMetrics` fires at most once in this long (04 §24: 1 s). */
  metricsMs: number;
  /** What the licence check needs (12 §7.4): the trusted keys, WebCrypto, and the page's host. */
  licence: {
    keys: readonly TrustedKey[];
    subtle(): SubtleCrypto | undefined;
    /** The page's host name, or `null` for an opaque origin (a file, a sandboxed frame). */
    host(): string | null;
  };
}

export interface Mounted {
  canvas: HTMLCanvasElement;
  /** Holds the canvas and the interface; theme tokens are read and set on it. */
  root: HTMLElement;
  /** The map's CSS size. */
  size(): [number, number];
  /** `--cavea-<name>` as computed on `root`, trimmed, colours as `#rrggbbaa`; "" where unset. */
  tokens(names: string[]): string[];
  /**
   * Calls `listener` when the page may have restyled the map (an attribute of
   * the container, `<html>` or `<body>` changed); returns the unsubscribe.
   */
  watch(listener: () => void): () => void;
  remove(): void;
}

export type ObjectStatus = "available" | "unavailable" | "selected";

/** The map `createSeatMap` resolves with (05 §6). Methods never throw; after `destroy` they do nothing. */
export interface SeatMap {
  /** The selection now, as `onSelectionChange` reports it. */
  getSelection(): Selection;
  /** As taps, all or nothing; unknown labels are reported through `onError`. */
  select(labels: string[]): void;
  /** Removes these objects from the selection, releasing any holds on them; unknown labels are reported through `onError`. */
  deselect(labels: string[]): void;
  /** Empties the selection, releasing any holds on it. */
  clearSelection(): void;
  /** Another theme, as the `theme` option takes; an invalid one is reported through `onError`. */
  setTheme(theme: Theme): void;
  /** Reads the `--cavea-*` custom properties again, after a change the map cannot see (05 §8). */
  refreshTheme(): void;
  /** Replaces the prices shown (05 §3.1). */
  setPricing(pricing: CategoryPricing[]): void;
  /** Messages, numbers and prices in another language (05 §3.4). */
  setLocale(locale: string): void;
  /** `fit`, a section label, a seat label, or bounds. Animated unless `animate` is false. */
  zoomTo(target: "fit" | string | Bounds, options?: { animate?: boolean }): void;
  /** Twice the zoom, about the centre. */
  zoomIn(): void;
  /** Half the zoom, about the centre. */
  zoomOut(): void;
  /** An object and its status now, or `null` for an unknown label. */
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
  "setFilter setMode setSpotlight setFloor selectBestAvailable showListView toImage";

type Callbacks = {
  [K in keyof SeatMapOptions as K extends `on${string}` ? K : never]-?: NonNullable<
    SeatMapOptions[K]
  >;
};

/** Frames in `onMetrics`' window (04 §24). */
const METRICS_FRAMES = 120;
const LEVELS: Metrics["level"][] = ["overview", "crossFade", "seats", "detail"];

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

/** Failures of the start that may succeed later are tried again this many times (05 §9). */
const RETRIES = 3;
/** The longest wait between tries, before jitter. */
const RETRY_CAP_MS = 30_000;

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
  let unwatch: Array<() => void> = [];
  let outstanding = 0;
  let locale = options.locale ?? globalThis.navigator?.language ?? "en";
  let localeGiven = options.locale !== undefined;
  let t = translator(locale, options.messages);
  let pricing = checkPricing(options.pricing ?? [], warn);
  const priceFailed = (error: unknown): void =>
    report(new CaveaError("callback_failed", `formatPrice threw: ${String(error)}`, false, error));
  const priceBook = () =>
    prices(pricing, options.formatPrice, options.currency, locale, t, priceFailed);
  let book = priceBook();
  /** The ticket type chosen for each selected object whose category has types. */
  const chosen = new Map<string, string>();
  let ui: ReturnType<typeof createUi> | undefined;
  let a11y: A11y | undefined;
  /** The keyboard focus shown, and the seat last moved clear of the controls. */
  let focusLabel: string | null = null;
  let nudged: string | null = null;
  /** The buyer's selection changes of the batch being delivered, said at its end. */
  let changes: { added: string[]; removed: string[] } | undefined;
  /** Labels a toast spoke of (taken, lost, refused): their removal is not said again. */
  const told = new Set<string>();
  /** Prices, strings and direction again, after `setPricing` or `setLocale`. */
  let rebuild: (() => void) | undefined;
  let queue: Promise<void> = Promise.resolve();

  /** Tells the host; an `onError` that throws is not called again for its own throw. */
  function report(error: CaveaError): void {
    if (error.code === "state_disconnected") {
      ui?.toast(t("reconnecting"));
    }
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
    return priced(seat);
  }

  /** `seat` with the chosen ticket type and the price from `pricing`, if any. */
  function priced({ ticketType: _type, price: _price, ...seat }: SeatObject): SeatObject {
    const out: SeatObject = seat;
    const type = chosen.get(seat.label);
    const price = book.price(seat.category.key, type);
    if (type !== undefined) {
      out.ticketType = type;
    }
    if (price !== undefined) {
      out.price = price;
    }
    return out;
  }

  /** Defaults the ticket type of each selected object whose category has types; forgets the rest. */
  function chooseDefaults(labels: Array<{ label: string; category: string }>): void {
    const keep = new Map(labels.map((o) => [o.label, o.category]));
    for (const [label, key] of chosen) {
      const types = book.types(keep.get(label) ?? "");
      if (!types.some((type) => type.key === key)) {
        chosen.delete(label);
      }
    }
    for (const [label, category] of keep) {
      const first = book.types(category)[0];
      if (first !== undefined && !chosen.has(label)) {
        chosen.set(label, first.key);
      }
    }
  }

  /** An object's details and public status code, from the view. */
  function details(
    label: string,
  ): (ObjectInfo & { status: number; place?: [number, number] }) | undefined {
    const json = view?.object(label);
    return json === undefined ? undefined : (JSON.parse(json) as ObjectDetails);
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
      announce();
    }
  }

  /**
   * Says the batch's selection changes (11 §5), once: a removal a toast
   * already explained (taken, lost, refused) is not said again.
   */
  function announce(): void {
    const batch = changes;
    changes = undefined;
    if (batch === undefined || destroyed) {
      return;
    }
    const removed = batch.removed.filter((label) => !told.has(label));
    for (const label of [...batch.added, ...batch.removed]) {
      told.delete(label);
    }
    if (batch.added.length + removed.length > 0) {
      a11y?.selection(batch.added, removed, selected.size);
    }
  }

  /**
   * Moves the camera the least that takes the focused seat at `at` out from
   * under a built-in control, once per seat (11 §3).
   */
  function clearControls([x, y]: [number, number]): void {
    const box = ui?.covered([x, y]);
    const size = mounted?.size();
    if (
      box === null ||
      box === undefined ||
      camera === undefined ||
      size === undefined ||
      nudged === focusLabel
    ) {
      return;
    }
    nudged = focusLabel;
    // The smallest shift to a side of the box that stays on the map.
    let best: [number, number] | undefined;
    for (const [dx, dy] of [
      [box[0] - x, 0],
      [box[2] - x, 0],
      [0, box[1] - y],
      [0, box[3] - y],
    ] as const) {
      const [nx, ny] = [x + dx, y + dy];
      if (nx >= 0 && nx <= size[0] && ny >= 0 && ny <= size[1]) {
        if (best === undefined || Math.abs(dx + dy) < Math.abs(best[0] + best[1])) {
          best = [dx, dy];
        }
      }
    }
    if (best === undefined) {
      return;
    }
    const [dx, dy] = best;
    const [cx, cy] = camera.centre;
    lookAt(cx - dx / camera.zoom, cy - dy / camera.zoom, camera.zoom);
  }

  function deliver(event: RendererEvent): void {
    switch (event.type) {
      case "ready":
        chart = event;
        ui?.legend(mapCategories());
        pushTheme();
        break;
      case "selectionChanged": {
        const before = selected;
        chooseDefaults(event.selected);
        selected = new Map(event.selected.map((o) => [o.label, toSeat(o)]));
        const added = [...selected.keys()].filter((label) => !before.has(label));
        const removed = [...before.keys()].filter((label) => !selected.has(label));
        changes ??= { added: [], removed: [] };
        changes.added.push(...added);
        changes.removed.push(...removed);
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
        ui?.selected(added.length === 1 ? (added[0] ?? null) : null);
        break;
      }
      case "selectionInvalid": {
        const rule = event.rule;
        call("onSelectionInvalid", rule, event.objects.map(lookup));
        // `min` is readiness: no toast speaks of its objects.
        for (const label of rule === "min" ? [] : event.objects) {
          told.add(label);
        }
        // `min` is readiness, not a refusal; `not_free` from a hold comes as `objectUnavailable`.
        if (rule !== "min") {
          ui?.toast(
            rule === "max"
              ? t("max", { count: max })
              : rule === "not_free"
                ? t("notFree")
                : rule === "hold_limit_exceeded"
                  ? t("holdLimit")
                  : t("holdFailed"),
          );
        }
        break;
      }
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
      case "objectFocus":
        call(
          event.type === "objectHover" ? "onObjectHover" : "onObjectFocus",
          event.object && toSeat(event.object),
          event.at,
        );
        ui?.hover(event.object?.label ?? null, event.at);
        if (event.type === "objectFocus") {
          focusLabel = event.object?.label ?? null;
          a11y?.focus(focusLabel);
          if (event.at !== null) {
            clearControls(event.at);
          }
        }
        break;
      case "objectUnavailable":
        told.add(event.label);
        call("onObjectUnavailable", lookup(event.label));
        ui?.toast(t("taken", { seat: event.label }));
        break;
      case "holdLost": {
        for (const label of event.labels) {
          told.add(label);
        }
        call("onHoldLost", event.labels.map(lookup));
        const expired = deadline !== null && serverNow() >= deadline - EXPIRED_SLACK_MS;
        if (expired) {
          deadline = null;
          call("onHoldExpired");
        }
        ui?.toast(expired ? t("holdExpired") : t("holdLost", { count: event.labels.length }));
        break;
      }
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
        ui?.camera(event.zoom, event.range);
        call("onCameraChange", { centre: event.centre, zoom: event.zoom, level: event.lod });
        break;
      case "error":
        report(
          new CaveaError(
            event.code === "glyphAtlas"
              ? "glyph_atlas_failed"
              : event.code === "image"
                ? "image_failed"
                : "engine_failed",
            event.message,
          ),
        );
        break;
    }
  }

  /** The scheme shown, whether it follows the system, and whether the system prefers dark. */
  let scheme: ThemeName = "light";
  let auto = false;
  let dark = false;
  /** `theme.tokens`, without the `--cavea-` prefix: set on the root for the interface too. */
  let themeTokens: Array<[string, string]> = [];
  /** The last theme the view was given, to skip a change that changes nothing. */
  let sent = "";

  /**
   * Gives the view the scheme and the `--cavea-*` tokens as the root computes
   * them: the page's custom properties under `theme.tokens`. A token of
   * `theme.tokens` with an unknown name is passed as given, for the view to
   * refuse (05 §3.4).
   */
  function pushTheme(): void {
    if (mounted === undefined || view === undefined) {
      return;
    }
    const names = [
      ...new Set([
        ...CANVAS_TOKENS,
        ...(chart?.categories ?? []).map((c) => `category-${c.key}`),
        ...themeTokens.map(([name]) => name).filter((name) => name.startsWith("category-")),
      ]),
    ];
    const values = mounted.tokens(names);
    const pairs = names.flatMap((name, i) => (values[i] ? [name, values[i] ?? ""] : []));
    for (const [name, value] of themeTokens) {
      if (!names.includes(name)) {
        pairs.push(name, value);
      }
    }
    const key = [scheme, ...pairs].join("\n");
    if (key !== sent) {
      sent = key;
      const v = view;
      engine(() => v.setTheme(scheme, pairs), "invalid_input");
      host?.update();
    }
  }

  function applyTheme(theme: Theme): void {
    const root = mounted?.root;
    const base = typeof theme === "string" ? theme : (theme.base ?? "light");
    for (const [name] of themeTokens) {
      root?.style.removeProperty(`--cavea-${name}`);
    }
    themeTokens =
      typeof theme === "string"
        ? []
        : Object.entries(theme.tokens).map(([name, value]) => [
            name.replace(/^--cavea-/, ""),
            String(value),
          ]);
    for (const [name, value] of themeTokens) {
      root?.style.setProperty(`--cavea-${name}`, value);
    }
    auto = base === "auto";
    setScheme(base === "auto" ? (dark ? "dark" : "light") : base);
  }
  function setScheme(name: ThemeName): void {
    scheme = name;
    mounted?.root.setAttribute("data-s", name);
    sent = "";
    pushTheme();
  }

  const mapCategories = (): Array<{ key: string; label: string; color: string; objects: number }> =>
    chart?.categories.map((c) => ({
      ...c,
      color: `#${c.color.map((v) => v.toString(16).padStart(2, "0")).join("")}`,
    })) ?? [];

  /** An object's details with its status (as `findObject`), from the view. */
  function find(label: string): TooltipObject | null {
    const info = details(label);
    if (info === undefined) {
      return null;
    }
    const status: ObjectStatus = selected.has(label)
      ? "selected"
      : info.status === 0
        ? "available"
        : "unavailable";
    return { ...(selected.get(label) ?? toSeat(info)), status };
  }

  /** The interface's language, direction and the strings the view draws. */
  function localise(): void {
    const dir = options.dir ?? "auto";
    const root = mounted?.root;
    // `auto` without a locale of the host's inherits the page's direction.
    if (dir === "auto" && !localeGiven) {
      root?.removeAttribute("dir");
    } else {
      root?.setAttribute("dir", dir === "auto" ? localeDirection(locale) : dir);
    }
    root?.setAttribute("lang", locale);
    a11y?.refresh();
    const v = view;
    engine(() => v?.setMessage("soldOut", t("soldOut")));
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

  /** Starts waiting for a retry of the error state (`load`). */
  let failing = 0;
  /** The error state without Retry, if the built-in loading state is on. */
  function notice(id: "offline" | "contextLost"): void {
    if (options.ui?.loading !== false) {
      void ui?.failed(id, false);
    }
  }
  /** The context has been lost longer than `contextLostMs` and has not returned. */
  let gone = false;
  /**
   * Ends the loading or error state once the map is drawn and no start
   * waits for Retry; a context still gone shows its notice instead.
   */
  function settled(): void {
    if (marks["first_frame"] !== undefined && failing === 0) {
      if (gone) {
        notice("contextLost");
      } else {
        ui?.loading(false);
      }
    }
  }

  let lostTimer: ReturnType<typeof setTimeout> | undefined;
  function onContext(restored: boolean): void {
    clearTimeout(lostTimer);
    timers.delete(lostTimer as ReturnType<typeof setTimeout>);
    if (!restored) {
      lostTimer = setTimeout(() => {
        report(
          new CaveaError("context_lost", `the WebGL context was lost for ${env.contextLostMs} ms`),
        );
        gone = true;
        // A start waiting for Retry keeps its error state and button.
        if (failing === 0) {
          notice("contextLost");
        }
      }, env.contextLostMs);
      timers.add(lostTimer);
    } else {
      gone = false;
      settled();
    }
  }

  /** Frame times of the last `METRICS_FRAMES` frames, a ring; kept only with `onMetrics`. */
  const frameMs = new Float64Array(METRICS_FRAMES);
  let framesTimed = 0;
  let sentAt = -Infinity;
  let metricsTimer: ReturnType<typeof setTimeout> | undefined;
  function sendMetrics(): void {
    timers.delete(metricsTimer as ReturnType<typeof setTimeout>);
    metricsTimer = undefined;
    const stats = view?.frameStats();
    if (stats === undefined || framesTimed === 0) {
      return;
    }
    sentAt = performance.now();
    const sorted = frameMs.slice(0, Math.min(framesTimed, METRICS_FRAMES));
    // oxlint-disable-next-line unicorn/no-array-sort -- sorts its own copy; the target lacks toSorted
    sorted.sort();
    const at = (q: number): number =>
      sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? 0;
    call("onMetrics", {
      frames: sorted.length,
      frameMs: { median: at(0.5), p95: at(0.95), max: at(1) },
      drawCalls: stats[0] ?? 0,
      visibleSeats: stats[1] ?? 0,
      level: LEVELS[stats[2] ?? -1] ?? null,
      backend: "webgl2",
      pixelRatio: stats[3] ?? 1,
      marks: { ...marks },
    });
  }
  /** Keeps a frame's time; sends at most once a second, the last frames included. */
  function timeFrame(ms: number): void {
    frameMs[framesTimed++ % METRICS_FRAMES] = ms;
    if (metricsTimer === undefined) {
      metricsTimer = setTimeout(
        sendMetrics,
        Math.max(0, sentAt + env.metricsMs - performance.now()),
      );
      timers.add(metricsTimer);
    }
  }

  /**
   * Decides the licence mark once and shows it (12 §7.3): from the server
   * through `caveaServer`, else from the `licence` option, else the
   * "Development licence" mark. Nothing here can fail the map (D-20).
   */
  async function showLicence(): Promise<void> {
    try {
      const fromAdapter = reported(adapter);
      if (fromAdapter !== undefined && options.licence !== undefined) {
        warn('option "licence" is ignored with caveaServer: the server\'s licence counts');
      }
      const standing: Standing =
        fromAdapter?.() ??
        (options.licence === undefined
          ? { kind: "invalid", reason: "no licence option" }
          : await check(options.licence, env.licence.keys, env.licence.subtle(), Date.now()));
      const decided = decide(standing, env.licence.host());
      if (decided.mark !== null && !destroyed) {
        warn(`the map shows the "${t(decided.mark)}" mark: ${decided.why} (12 §7.3)`);
        ui?.mark(decided.mark);
      }
    } catch {
      // A licence problem never stops a sale: no mark.
    }
  }

  function teardown(): void {
    destroyed = true;
    for (const timer of timers) {
      clearTimeout(timer);
    }
    timers.clear();
    abort.abort();
    for (const off of unwatch.splice(0)) {
      off();
    }
    ui?.destroy();
    engine(() => adapter.disconnect(), "state_failed");
    host?.detach();
    view?.free();
    view = undefined;
    mounted?.remove();
  }

  /**
   * Waits `ms`, then while the page is hidden or offline (05 §9); with
   * `show`, before the first frame, offline shows as the error state.
   */
  async function pause(ms: number, show: boolean): Promise<void> {
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        timers.delete(timer);
        resolve();
      }, ms);
      timers.add(timer);
    });
    const shown = show && !env.online() && marks["first_frame"] === undefined && ui !== undefined;
    if (shown) {
      notice("offline");
    }
    await env.usable(abort.signal);
    if (shown) {
      ui?.loading(true);
    }
  }

  /**
   * Runs `run` until it succeeds (05 §9): a failure that cannot succeed
   * (`permanent`) is thrown at once, others are tried again `RETRIES` times
   * after a capped, jittered backoff, never while the page is hidden or
   * offline.
   */
  async function retrying<T>(run: () => T | Promise<T>, show = true): Promise<T> {
    for (let tries = 0; ; tries += 1) {
      try {
        return await attempt(run);
      } catch (error) {
        if (destroyed || permanent(error) || tries >= RETRIES) {
          throw error;
        }
        const ms = Math.min(RETRY_CAP_MS, env.retryMs * 2 ** tries) * (0.5 + Math.random());
        await pause(ms, show);
      }
    }
  }

  /**
   * As `retrying`; then, with the error state, a failure that may yet
   * succeed is reported and offered again (D-48); otherwise it is fatal.
   */
  async function load<T>(code: CaveaErrorCode, run: () => T | Promise<T>): Promise<T> {
    for (let retried = false; ; retried = true) {
      try {
        const value = await retrying(run);
        if (retried) {
          settled();
        }
        return value;
      } catch (error) {
        if (ui === undefined || options.ui?.loading === false || destroyed || permanent(error)) {
          throw new CaveaError(code, String(error), true, error);
        }
        report(new CaveaError(code, String(error), false, error));
        // Shown until Retry, even over a map already drawn.
        failing += 1;
        try {
          await ui.failed(env.online() ? "loadFailed" : "offline");
        } finally {
          failing -= 1;
        }
        ui.loading(true);
      }
    }
  }

  // The four fetches start here, together (04 §4).
  const wasm = load("wasm_unavailable", () => env.wasm(options.assets));
  const given = options.chart ?? adapter.chart;
  const chartBytes =
    given === undefined
      ? Promise.reject(
          new CaveaError(
            "chart_fetch_failed",
            "no chart: pass options.chart or an adapter with chart()",
            true,
          ),
        )
      : load("chart_fetch_failed", async () => {
          const source = options.chart ?? (await adapter.chart?.(abort.signal));
          return source === undefined || "bytes" in source
            ? bytesOf(source?.bytes ?? new Uint8Array())
            : env.fetchBytes(source.url, abort.signal);
        });
  const state = load("state_connect_failed", () => adapter.connect(ctx));
  const atlas = retrying(() => env.atlas(options.assets), false);
  // A fatal failure of any, or the host's abort, ends the start at once.
  let ready = false;
  const failed = new Promise<never>((_, reject) => {
    for (const promise of [wasm, chartBytes, state]) {
      promise.catch(reject);
    }
    const signal = options.signal;
    const stop = (): void => {
      if (ready) {
        map.destroy();
      } else {
        reject(new CaveaError("aborted", "options.signal was aborted", true, signal?.reason));
      }
    };
    if (signal?.aborted) {
      stop();
    }
    signal?.addEventListener("abort", stop);
    unwatch.push(() => signal?.removeEventListener("abort", stop));
  });
  // Each is awaited or handed over below; none is an unhandled rejection meanwhile.
  for (const promise of [wasm, chartBytes, state, atlas, failed]) {
    promise.catch(() => undefined);
  }
  const step = <T>(promise: Promise<T>): Promise<T> => Promise.race([promise, failed]);
  let mounted: ReturnType<Env["mount"]> | undefined;
  try {
    mounted = env.mount(element);
    const { canvas, root } = mounted;
    if (typeof options.reducedMotion === "boolean") {
      root.setAttribute("data-m", options.reducedMotion ? "reduce" : "full");
    }
    const uiHost: UiHost = {
      t,
      prices: book,
      options: options.ui ?? {},
      size: () => mounted?.size() ?? [0, 0],
      zoom: (how) =>
        how === "fit" ? fly(undefined, undefined, false, true) : zoomBy(how === "in" ? 2 : 0.5),
      describe: find,
      color: (key) => mapCategories().find((c) => c.key === key)?.color,
      chooseTicketType,
      failed: (error) =>
        report(
          new CaveaError("callback_failed", `ui.tooltip threw: ${String(error)}`, false, error),
        ),
    };
    const shell = createUi(root, canvas, uiHost);
    ui = shell;
    a11y = createA11y(canvas, root, {
      get t() {
        return t;
      },
      get prices() {
        return book;
      },
      describe(label) {
        const object = find(label);
        const place = details(label)?.place;
        return object && place ? { ...object, place } : object;
      },
    });
    rebuild = () => {
      book = priceBook();
      uiHost.t = t;
      uiHost.prices = book;
      chooseDefaults(
        [...selected.values()].map((o) => ({ label: o.label, category: o.category.key })),
      );
      selected = new Map([...selected].map(([label, o]) => [label, priced(o)]));
      ui?.refresh();
      localise();
    };
    localise();
    unwatch.push(
      env.darkScheme((on) => {
        dark = on;
        if (auto) {
          setScheme(on ? "dark" : "light");
        } else {
          pushTheme();
        }
      }),
      mounted.watch(pushTheme),
    );
    const makeView = await step(wasm);
    mark("wasm_ready");
    let v: EngineView;
    try {
      v = makeView(canvas);
    } catch (error) {
      throw new CaveaError("webgl_unavailable", String(error), true, error);
    }
    mark("gpu_ready");
    view = v;
    v.setRules(max, min);
    if (adapter.hold === undefined) {
      v.setManualSession(true);
    }
    const attachOptions: AttachOptions = {
      gestureHandling: options.gestureHandling ?? "greedy",
      glyphAtlas: atlas,
      onEvents,
      onFrame: (_time, ms) => {
        if (chart !== undefined && marks["first_frame"] === undefined) {
          mark("first_frame");
          settled();
        }
        // The first frame drawn with the first state applied.
        if (marks["state_ready"] !== undefined && marks["interactive"] === undefined) {
          mark("interactive");
          // The licence check, after this frame and never before it (12 §7.4).
          const timer = setTimeout(() => {
            timers.delete(timer);
            void showLicence();
          }, 0);
          timers.add(timer);
        }
        if (options.onMetrics !== undefined) {
          timeFrame(ms);
        }
      },
      onContext,
      timeFrames: options.onMetrics !== undefined,
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
    const image = options.assets?.image ?? adapter.image?.bind(adapter);
    if (image !== undefined) {
      attachOptions.images = image;
    }
    if (typeof options.reducedMotion === "boolean") {
      attachOptions.reducedMotion = options.reducedMotion;
    }
    host = env.attach(v, canvas, attachOptions);
    applyTheme(options.theme ?? "auto");
    localise();

    const bytes = await step(chartBytes);
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

    const first = await step(state);
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
    refreshTheme() {
      if (live() !== undefined) {
        pushTheme();
      }
    },
    setPricing(next) {
      if (live() === undefined) {
        return;
      }
      if (Array.isArray(next)) {
        pricing = checkPricing(next, warn);
        rebuild?.();
      } else {
        report(new CaveaError("invalid_input", `invalid pricing: ${String(next)}`));
      }
    },
    setLocale(next) {
      if (live() === undefined) {
        return;
      }
      if (isLocale(next)) {
        locale = next;
        localeGiven = true;
        t = translator(locale, options.messages);
        rebuild?.();
      } else {
        report(new CaveaError("invalid_input", `invalid locale: ${String(next)}`));
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
    findObject: (label) => (live() === undefined ? null : find(label)),
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
    listCategories: () => (live() === undefined ? [] : mapCategories()),
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
  /** The buyer chose a ticket type for a selected object, in the tooltip. */
  function chooseTicketType(label: string, key: string): void {
    const object = selected.get(label);
    if (object !== undefined && book.types(object.category.key).some((type) => type.key === key)) {
      chosen.set(label, key);
      selected.set(label, priced(object));
      call("onSelectionChange", selection());
      ui?.selected(null);
    }
  }
  function zoomBy(factor: number): void {
    if (live() !== undefined && camera !== undefined) {
      lookAt(...camera.centre, camera.zoom * factor);
    }
  }
  /** Flies to `scale` CSS pixels per chart unit about the chart point (`x`, `y`). */
  function lookAt(x: number, y: number, scale: number): void {
    const size = mounted?.size() ?? [0, 0];
    // The engine fits bounds inside a margin of `FIT_PADDING_PX` (24) on each side.
    const room = (px: number): number => Math.max(px - 48, 1) / scale;
    const [width, height] = [room(size[0]), room(size[1])];
    fly({ x: x - width / 2, y: y - height / 2, width, height }, undefined, false, true);
  }
  for (const name of LATER_METHODS.split(" ")) {
    (map as unknown as Record<string, () => void>)[name] = () =>
      warn(`${name}() is not supported yet`);
  }

  ready = true;
  call("onReady", {
    chart: { name: chart?.name ?? "", seats: chart?.seats ?? 0, objects: chart?.objects ?? 0 },
    backend: "webgl2",
    marks: { ...marks },
  });
  return map;
}
