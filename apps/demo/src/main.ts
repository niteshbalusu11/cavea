// A seat map built with the public API only: `createSeatMap` and a
// `customState` adapter that plays a ticketing backend in the page. Build the
// WASM with `cargo xtask build-web --demo` first: the synthetic charts come
// from its debug-only exports.
//
// Query parameters. Chart: `seats` (default 100000), `seed` (default 1),
// `preset` (a `cavea-testkit` preset such as `arena` instead of the stadium),
// `shape` (0 circle, 1 rounded, 2 chair), `script=1` (section labels and text
// objects in other scripts, right-to-left ones among them; some sections'
// rows prefixed "Ряд " and seats numbered a, b, c: labels the baked atlas
// lacks), all compiled in the browser; or
// `chart=url` to fetch a compiled chart, which the map then fetches in
// parallel with the WASM, the atlas and the fake host's state. `image=url`:
// every background image the chart asks for (04 §7) is fetched from there
// (`assets.image`); with `server=`, images come from the server by default.
// Map: `theme`
// (light, dark, high-contrast, auto; default auto), `gestures=greedy|
// cooperative|none` (cooperative puts the map in a scrolling page),
// `reducedMotion=1|0`, `camera=x:y:width:height` (initial view in chart
// units), `max` and `min` (selection rules, default 4 and 0), `meter=1` (a
// frame-time meter and the timing marks, from `onMetrics`: it updates only
// when the map draws, so an idle map stays idle).
//
// Fake host: `sold` (fraction of seats unavailable at start, in runs; default
// 0.3) or `mark=start:count` (exactly that `idx` range); `stateDelay=ms`
// holds the first state back; each hold is answered after `holdMs` (default
// 400) ms, and `fail=n` refuses every n-th as taken by someone else;
// `manual=1` gives the adapter no hold or release, so selecting holds
// nothing. `crowd=ms` takes `crowdSize` (default 20) random seats and frees
// as many every `ms` ms; `snipe=1` also takes a selected seat the buyer does
// not hold yet, so it is knocked out.
//
// Built-in interface (05 §7): `prices=1` gives the chart's first category two
// ticket types and the others one price each, through `setPricing` once the
// categories are known (`currency`, default GBP; `locale`); `tooltip=host`
// renders the tooltip from the page; `hide=zoomControls,legend,tooltip,
// toasts,loading` turns elements off; `messages=fr` overrides some strings;
// `dir=rtl` makes the page right to left; `brand=1` sets theme tokens as
// custom properties on the container (`caveaDemo.brand(on)` toggles them
// live); `width=px` narrows the container; `quiet=1` hides the readouts.
//
// Loading (05 §10): `preload=1` calls `preloadSeatMap` (with `chart=url`'s
// chart) as the page starts; `wait=ms` creates the map that long after, with
// or without it, to compare `first_frame`.
//
// Real server instead of the fake host: `server=<baseUrl>&key=<pk_…>&event=<key>`,
// with `transport=auto|ws|sse|poll` and `session=start|manual|none` (05 §4);
// the chart is the event's. Needs no `--demo` build.
//
// `window.caveaDemo` has the map (`map.select([...])`, `map.zoomTo(...)`,
// `map.destroy()` from the console), the seat `labels`, the callback log in `events`, and
// `take(label)`, `crowd(on)`, `resnapshot()` and `second(hidden)` (a second map
// on the same page, sharing the WASM).
import {
  caveaServer,
  createSeatMap,
  customState,
  preloadSeatMap,
  loadRenderer,
  type AdapterContext,
  type GestureHandling,
  type Metrics,
  type SeatMap,
  type SeatMapOptions,
  type Snapshot,
  type Theme,
} from "@cavea/renderer";
// Debug-only exports, present in `--demo` builds; never part of the SDK.
import * as wasm from "../../../packages/renderer/wasm/cavea_web.js";

interface DemoStatics {
  demoChart(seats: number, seed: number, shape: number, preset: string, script: number): Uint8Array;
  demoLabels(ccf: Uint8Array): string[];
}
const demo = wasm.View as unknown as Partial<DemoStatics>;
/** Seat label draws, dynamic text and row label draws, all draws, atlas bytes, strings rasterised. */
const textStats = (wasm as unknown as { demoTextStats?: () => Uint32Array }).demoTextStats;

function element<T extends HTMLElement>(id: string, type: new () => T): T {
  const node = document.getElementById(id);
  if (!(node instanceof type)) {
    throw new Error(`#${id} is missing from index.html`);
  }
  return node;
}
const status = element("status", HTMLOutputElement);
const selectionLine = element("selection", HTMLOutputElement);
const container = element("map", HTMLDivElement);

const lines: string[] = [];
function report(line: string): void {
  lines.push(line);
  if (lines.length > 8) {
    lines.splice(4, lines.length - 8);
  }
  status.textContent = lines.join("\n");
  console.log(`[cavea-demo] ${line}`);
}

const params = new URLSearchParams(location.search);
if (params.get("preload") === "1") {
  const url = params.get("chart");
  void preloadSeatMap(url === null ? {} : { chart: { url } });
}
const num = (name: string, fallback: number): number => Number(params.get(name) ?? fallback);
const chartUrl = params.get("chart");
const preset = params.get("preset") ?? "";
const sold = num("sold", 0.3);
const mark = params.get("mark")?.split(":").map(Number);
const camera = params.get("camera")?.split(":").map(Number);
const themeParam = params.get("theme") ?? "auto";
const theme = (
  ["light", "dark", "high-contrast"].includes(themeParam) ? themeParam : "auto"
) as Theme;
const gesturesParam = params.get("gestures");
const gestures: GestureHandling =
  gesturesParam === "cooperative" || gesturesParam === "none" ? gesturesParam : "greedy";
const max = num("max", 4);
const min = num("min", 0);
const manual = params.get("manual") === "1";
const failEvery = num("fail", 0);
const holdMs = num("holdMs", 400);
const stateDelay = num("stateDelay", 0);
const crowdMs = num("crowd", 0);
const crowdSize = num("crowdSize", 20);
const snipe = params.get("snipe") === "1";
const server = params.get("server");
const hide = new Set(params.get("hide")?.split(",") ?? []);
const currency = params.get("currency") ?? "GBP";
const locale = params.get("locale");
const brand = (on: boolean): void => {
  const tokens: Record<string, string> = {
    "--cavea-bg": "#fff8f0",
    "--cavea-fg": "#2b1a12",
    "--cavea-fg-muted": "#6b4a3a",
    "--cavea-ring-focus": "#c2410c",
    "--cavea-seat-selected-fill": "#7c2d12",
    "--cavea-radius": "2px",
    "--cavea-font": "Georgia, serif",
  };
  for (const [name, value] of Object.entries(tokens)) {
    if (on) {
      container.style.setProperty(name, value);
    } else {
      container.style.removeProperty(name);
    }
  }
};
if (params.get("brand") === "1") {
  brand(true);
}
if (params.get("dir") === "rtl") {
  document.documentElement.dir = "rtl";
}
if (params.has("width")) {
  container.style.cssText = `position:relative;width:${num("width", 320)}px;height:90vh;margin:0 auto`;
}
if (params.get("quiet") === "1") {
  status.hidden = true;
  selectionLine.hidden = true;
}
const meter = params.get("meter") === "1" ? document.createElement("pre") : undefined;
if (meter !== undefined) {
  meter.id = "meter";
  meter.style.cssText =
    "position:fixed;right:8px;top:8px;margin:0;padding:6px 8px;font:12px/1.4 ui-monospace,monospace;" +
    "background:rgb(0 0 0/.75);color:#fff;border-radius:4px;pointer-events:none;z-index:10";
  meter.textContent = "meter: no frame yet";
  document.body.append(meter);
}
const fixed = (v: number): string => v.toFixed(2);
document.body.dataset["theme"] = theme as string;
if (gestures === "cooperative") {
  document.body.classList.add("scrolling");
}
const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

let rngState = num("seed", 1) >>> 0 || 1;
function random(): number {
  rngState ^= rngState << 13;
  rngState ^= rngState >>> 17;
  rngState ^= rngState << 5;
  return (rngState >>> 0) / 2 ** 32;
}

/** The chart's bytes and seat labels, when compiled here. */
async function generated(): Promise<{ ccf: Uint8Array; labels: string[] }> {
  await loadRenderer();
  if (demo.demoChart === undefined || demo.demoLabels === undefined) {
    throw new Error(
      "This WASM has no demo exports. Run `cargo xtask build-web --demo` and reload.",
    );
  }
  const ccf = demo.demoChart(
    num("seats", 100_000),
    num("seed", 1),
    num("shape", 0),
    preset,
    num("script", 0),
  );
  return { ccf, labels: demo.demoLabels(ccf) };
}

/** The fake host: public state and the buyer's holds, by label, as a custom backend keeps them. */
let labels: string[] = [];
const taken = new Set<string>();
const held = new Set<string>();
let ctx: AdapterContext | undefined;
let holds = 0;
const expiry = (): number => Date.now() + 15 * 60_000;
const snapshot = (): Snapshot => ({
  kind: "map",
  unavailable: [...taken],
  mine: [...held],
  expiresAt: expiry(),
});

function soldRuns(fraction: number): void {
  for (let at = 0; at < labels.length;) {
    const count = 1 + Math.floor(random() * 24);
    at += Math.floor((random() * 2 * count * (1 - fraction)) / Math.max(fraction, 1e-3));
    for (const label of labels.slice(at, at + count)) {
      taken.add(label);
    }
    at += count;
  }
}

const adapter = customState({
  async connect(context) {
    ctx = context;
    if (chartUrl !== null) {
      // As a real host fetches its state; here the labels come from the chart.
      const response = await fetch(chartUrl, { signal: context.signal });
      const ccf = new Uint8Array(await response.arrayBuffer());
      await loadRenderer();
      labels = demo.demoLabels?.(ccf) ?? [];
    }
    if (mark !== undefined && mark.length === 2) {
      const [from = 0, count = 0] = mark;
      labels.slice(from, from + count).forEach((label) => taken.add(label));
    } else if (sold > 0) {
      soldRuns(sold);
    }
    await sleep(stateDelay);
    report(
      `state: ${taken.size} of ${labels.length} seats unavailable (after ${stateDelay} ms delay)`,
    );
    return snapshot();
  },
  ...(manual
    ? {}
    : {
        async hold(requested: string[]) {
          await sleep(holdMs);
          holds += 1;
          for (const label of requested) {
            taken.add(label);
          }
          ctx?.applyDelta({ unavailable: requested });
          if (failEvery > 0 && holds % failEvery === 0) {
            const failed = requested.map((label) => ({ label, reason: "object_not_free" }));
            return { ok: false as const, code: "object_not_free", failed };
          }
          requested.forEach((label) => held.add(label));
          ctx?.setMine([...held], expiry());
          return { ok: true as const };
        },
        async release(released: string[]) {
          await sleep(holdMs / 2);
          for (const label of released) {
            held.delete(label);
            taken.delete(label);
          }
          ctx?.setMine([...held], expiry());
          ctx?.applyDelta({ available: released });
        },
      }),
  resync: () => ctx?.applySnapshot(snapshot()),
  disconnect: () => crowd(false),
});

let map: SeatMap | undefined;
let crowdTimer = 0;
function crowdTick(): void {
  const unavailable: string[] = [];
  const available: string[] = [];
  for (let n = 0; n < crowdSize * 4 && unavailable.length + available.length < crowdSize * 2; n++) {
    const label = labels[Math.floor(random() * labels.length)];
    if (label !== undefined && !held.has(label)) {
      (unavailable.length < crowdSize ? unavailable : available).push(label);
    }
  }
  if (snipe) {
    const target = map?.getSelection().objects.find((o) => !held.has(o.label));
    if (target !== undefined) {
      unavailable.push(target.label);
    }
  }
  unavailable.forEach((label) => taken.add(label));
  available.forEach((label) => taken.delete(label));
  ctx?.applyDelta({ unavailable, available });
}
function crowd(on: boolean): void {
  clearInterval(crowdTimer);
  crowdTimer = on && crowdMs > 0 ? window.setInterval(crowdTick, crowdMs) : 0;
}

const events: unknown[][] = [];
const log =
  (name: string) =>
  (...args: unknown[]): void => {
    events.push([name, ...args]);
  };
let hover = "";
let lastNote = "";
function note(line: string): void {
  lastNote = line;
  console.log(`[cavea-demo] ${line}`);
  showSelection();
}
/** The text readout: label draw calls of the last frame and the dynamic atlas. */
let textLine = "";
function showText(): void {
  const [seat = 0, text = 0, all = 0, bytes = 0, strings = 0] = textStats?.() ?? [];
  textLine = `draws ${all} (seat labels ${seat}, text ${text}); atlas ${bytes / 1024} KiB, ${strings} strings rasterised`;
  showSelection();
}
function showSelection(): void {
  const selection = map?.getSelection();
  const at = selection?.expiresAt ?? null;
  const expires = at === null ? "none" : new Date(at).toLocaleTimeString();
  selectionLine.textContent =
    `selected ${selection?.objects.length ?? 0}/${max}: ${selection?.objects.map((o) => o.label).join(" ") ?? ""}\n` +
    `held: ${server === null ? held.size : selection?.holdToken ? "by token" : "none"}, expire ${expires}; checkout ${selection?.valid === false ? "not ready" : "ready"}; ` +
    `hover: ${hover || "none"}\n${lastNote}\n${textLine}`;
}

const options: SeatMapOptions = {
  state: adapter,
  selection: { max, min },
  theme,
  gestureHandling: gestures,
  currency,
  ...(locale === null ? {} : { locale }),
  ui: {
    zoomControls: !hide.has("zoomControls"),
    legend: !hide.has("legend"),
    toasts: !hide.has("toasts"),
    loading: !hide.has("loading"),
    tooltip: hide.has("tooltip")
      ? false
      : params.get("tooltip") === "host"
        ? (object) => {
            // The page's own element: its styles are the page's, not the map's.
            const card = document.createElement("div");
            card.style.cssText =
              "font:600 13px system-ui;color:#0b3d2e;background:#d1fae5;padding:6px 8px";
            card.textContent = `${object.label} · ${object.category.label}${object.price === undefined ? "" : ` · ${(object.price / 100).toFixed(2)}`}`;
            return card;
          }
        : true,
  },
  ...(params.get("messages") === "fr"
    ? {
        messages: {
          zoomIn: "Zoomer",
          zoomOut: "Dézoomer",
          zoomFit: "Voir tout le plan",
          legend: "Légende",
          available: "Disponible",
          selected: "Sélectionné",
          unavailable: "Indisponible",
          max: { one: "Un seul siège possible.", other: "{count} sièges au maximum." },
          taken: "{seat} vient d’être pris.",
          soldOut: "Complet",
        },
      }
    : {}),
  onReady(info) {
    log("ready")(info);
    report(`ready: ${info.chart.seats} seats; marks ${JSON.stringify(info.marks)}`);
    crowd(true);
  },
  onSelectionChange(selection) {
    // Whether there is a hold token, never the token.
    log("selectionChange")(
      selection.objects.map((o) => o.label),
      selection.valid,
      selection.holdToken !== null,
      selection.expiresAt,
    );
    note(`selection: ${selection.objects.map((o) => o.label).join(", ") || "empty"}`);
  },
  onObjectSelect: log("objectSelect"),
  onObjectDeselect: log("objectDeselect"),
  onSelectionInvalid(rule, objects) {
    log("selectionInvalid")(
      rule,
      objects.map((o) => o.label),
    );
    note(`refused (${rule}): ${objects.map((o) => o.label).join(", ")}`);
  },
  onSelectionValid: log("selectionValid"),
  onHoldPending: log("holdPending"),
  onHoldSettled: log("holdSettled"),
  onHoldFailed(failed) {
    log("holdFailed")(failed.map((f) => [f.object.label, f.reason]));
    note(`hold failed: ${failed.map((f) => `${f.object.label} (${f.reason})`).join(", ")}`);
  },
  onHoldLost: (objects) => note(`hold lost: ${objects.map((o) => o.label).join(", ")}`),
  onHoldExpiring(seconds) {
    log("holdExpiring")(seconds);
    note(`hold expires in ${seconds} s`);
  },
  onHoldExpired() {
    log("holdExpired")();
    note("hold expired");
  },
  onObjectUnavailable(object) {
    log("objectUnavailable")(object.label);
    note(`${object.label} was taken`);
  },
  onObjectHover(object) {
    hover = object?.label ?? "";
    showSelection();
  },
  onCameraChange: log("cameraChange"),
  ...(meter === undefined
    ? {}
    : {
        onMetrics(m: Metrics) {
          const marks = Object.entries(m.marks)
            .map(([name, at]) => `${name.padEnd(13)} ${at.toFixed(1)} ms`)
            .join("\n");
          meter.textContent =
            `frame ms over ${m.frames}: median ${fixed(m.frameMs.median)}, p95 ${fixed(m.frameMs.p95)}, max ${fixed(m.frameMs.max)}\n` +
            `draw calls ${m.drawCalls}, seats ${m.visibleSeats}, ${m.level ?? "none"}, ${m.backend} at ${m.pixelRatio}x\n${marks}`;
        },
      }),
  onError(error) {
    log("error")(error.code, error.message);
    report(`error ${error.code}: ${error.message}`);
  },
};
if (params.has("reducedMotion")) {
  options.reducedMotion = params.get("reducedMotion") === "1";
}
if (camera !== undefined && camera.length === 4) {
  const [x = 0, y = 0, width = 1, height = 1] = camera;
  options.initialView = { x, y, width, height };
}

/** Another map on the page with fixed state, sharing the loaded WASM; `hidden` creates it in a hidden box. */
async function second(hidden = false): Promise<SeatMap> {
  const box = document.createElement("div");
  box.id = "second";
  box.hidden = hidden;
  document.body.append(box);
  const chart = options.chart ?? { url: chartUrl ?? "" };
  return createSeatMap(box, { chart, theme: "dark" });
}

try {
  if (server !== null) {
    const pick = <T extends string>(name: string, values: T[]): T | undefined =>
      values.find((v) => v === params.get(name));
    const transport = pick("transport", ["auto", "ws", "sse", "poll"]);
    const session = pick("session", ["start", "manual", "none"]);
    options.state = caveaServer({
      baseUrl: server,
      publishableKey: params.get("key") ?? "",
      event: params.get("event") ?? "",
      ...(transport === undefined ? {} : { transport }),
      ...(session === undefined ? {} : { session }),
    });
  } else if (chartUrl === null) {
    const chart = await generated();
    labels = chart.labels;
    options.chart = { bytes: chart.ccf };
  } else {
    options.chart = { url: chartUrl };
  }
  const image = params.get("image");
  if (image !== null) {
    options.assets = { image: () => image };
  }
  await new Promise((resolve) => setTimeout(resolve, Number(params.get("wait") ?? 0)));
  map = await createSeatMap(container, options);
  if (params.get("prices") === "1") {
    map.setPricing(
      map.listCategories().map((c, i) =>
        i === 0
          ? {
              category: c.key,
              ticketTypes: [
                { key: "adult", label: "Adult", price: 9500 },
                { key: "child", label: "Child", price: 6000 },
              ],
            }
          : { category: c.key, price: 7500 - 1500 * i },
      ),
    );
  }
  // A timer for the readout only; the map's frame loop stays idle.
  setInterval(showText, 500);
  Object.assign(window, {
    caveaDemo: {
      map,
      labels,
      events,
      second,
      crowd,
      brand,
      textStats: () => [...(textStats?.() ?? [])],
      resnapshot: () => ctx?.applySnapshot(snapshot()),
      take(label: string) {
        taken.add(label);
        ctx?.applyDelta({ unavailable: [label] });
      },
    },
  });
} catch (error) {
  report(`createSeatMap failed: ${String(error)}`);
}
