// The browser `Env` for `createSeatMap`. Nothing here runs at import, so the
// package can be imported where there is no DOM (server-side rendering).
import init, { View, type InitInput } from "../wasm/cavea_web.js";
import { loadGlyphAtlas } from "./atlas.ts";
import { CaveaError, httpError } from "./errors.ts";
import { attach } from "./host.ts";
import type { SeatMapOptions } from "./options.ts";
import type { Env } from "./seatmap.ts";
import { isColourToken, schemeCss } from "./theme.ts";
import { UI_CSS } from "./ui.ts";
import type { EngineView } from "./view.ts";

/** A hold or release without an answer this long has failed. */
const HOLD_TIMEOUT_MS = 20_000;

let loading: Promise<unknown> | undefined;

/**
 * Fetches, compiles and instantiates the WASM once per page (streaming
 * compilation when given a URL or `Response`); every map shares the module.
 * The first call's source wins. A failed load is not kept: the next call
 * tries again.
 */
export function loadWasm(source?: InitInput): Promise<unknown> {
  if (typeof WebAssembly === "undefined") {
    return Promise.reject(new CaveaError("wasm_unavailable", "WebAssembly is not available", true));
  }
  const input = source ?? new URL("../wasm/cavea_web_bg.wasm", import.meta.url);
  // Fetched here, so an HTTP failure keeps its status (05 §9: a 404 is not retried).
  loading ??= (
    typeof input === "string" || input instanceof URL
      ? fetchOk(input).then((r) => init({ module_or_path: r }))
      : init({ module_or_path: input })
  ).catch((error: unknown) => {
    loading = undefined;
    throw error;
  });
  return loading;
}

/** Decoded atlases and preloaded charts by URL, shared by every map; a failure is not kept. */
const shared = new Map<string, Promise<Uint8Array>>();
function once(key: string, start: () => Promise<Uint8Array>): Promise<Uint8Array> {
  let promise = shared.get(key);
  if (promise === undefined) {
    promise = start();
    shared.set(key, promise);
    promise.catch(() => shared.delete(key));
  }
  return promise;
}

/** `url`'s response; one that is not OK throws with its status. */
async function fetchOk(url: string | URL, signal?: AbortSignal): Promise<Response> {
  const response = await fetch(url, signal === undefined ? {} : { signal });
  if (!response.ok) {
    throw httpError(String(url), response.status);
  }
  return response;
}

async function fetchBytes(url: string | URL, signal?: AbortSignal): Promise<Uint8Array> {
  return new Uint8Array(await (await fetchOk(url, signal)).arrayBuffer());
}

const href = (url: string | URL): string => new URL(url, document.baseURI).href;
const atlas = (assets: SeatMapOptions["assets"]): Promise<Uint8Array> => {
  const url = asset(assets, "glyphs.png");
  return once(`atlas ${url === undefined ? "" : href(url)}`, () => loadGlyphAtlas(url));
};

/**
 * Starts fetching and compiling what a map needs (05 §10): the WASM, the
 * glyph atlas and, given a chart URL, the chart; later maps use them. Does
 * nothing without a DOM; never rejects (a failure is reported by the map
 * that needs it, which tries again).
 */
export function preload(
  options: Pick<SeatMapOptions, "assets"> & { chart?: { url: string | URL } } = {},
): Promise<void> {
  if (typeof document === "undefined") {
    return Promise.resolve();
  }
  const { assets, chart } = options;
  const jobs: Array<Promise<unknown>> = [
    loadWasm(assets?.wasm ?? asset(assets, "cavea_web_bg.wasm")),
    atlas(assets),
  ];
  if (chart?.url !== undefined) {
    const url = chart.url;
    jobs.push(once(href(url), () => fetchBytes(url)));
  }
  return Promise.allSettled(jobs).then(() => undefined);
}

/** `name` under `assets.baseUrl`, which may be relative to the page. */
function asset(assets: SeatMapOptions["assets"], name: string): URL | undefined {
  const base = assets?.baseUrl;
  return base === undefined ? undefined : new URL(name, new URL(base, document.baseURI));
}

// The page's own rules for the element win over `:host`; `hidden` still hides.
const css = (): string =>
  ":host{display:block;position:relative}:host([hidden]){display:none}" +
  ".r canvas{position:absolute;inset:0;width:100%;height:100%;display:block;outline:none}" +
  // The map's own focus, from the keyboard, before a seat has one (11 §3.1).
  ".r canvas:focus-visible{outline:2px solid var(--_ring-focus);outline-offset:-2px}" +
  schemeCss() +
  UI_CSS;

/**
 * One constructed stylesheet for every map on the page: adopted, it needs no
 * `style-src 'unsafe-inline'` under a Content Security Policy (05 §10).
 */
let sheet: CSSStyleSheet | undefined;

type Context2D = CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;
let probe: Context2D | null | undefined;

/**
 * A CSS colour as `#rrggbbaa`, by painting one pixel, so any colour the
 * browser knows (`rgb()`, `oklch()`, names) reaches the view; a value it
 * does not take is returned as given, for the view to refuse.
 */
function hex(value: string): string {
  if (/^#([\da-f]{3}|[\da-f]{6}|[\da-f]{8})$/i.test(value)) {
    return value;
  }
  probe ??= (
    typeof OffscreenCanvas === "undefined"
      ? document.createElement("canvas")
      : new OffscreenCanvas(1, 1)
  ).getContext("2d", { willReadFrequently: true }) as Context2D | null;
  if (probe === null) {
    return value;
  }
  // Two sentinels: a value the context refuses leaves both unchanged; a
  // valid one may equal one of them (black, white), never both.
  const parsed = ["#000", "#fff"].some((sentinel) => {
    probe!.fillStyle = sentinel;
    probe!.fillStyle = value;
    return probe!.fillStyle !== (sentinel === "#000" ? "#000000" : "#ffffff");
  });
  if (!parsed) {
    return value;
  }
  probe.clearRect(0, 0, 1, 1);
  probe.fillRect(0, 0, 1, 1);
  return `#${[...probe.getImageData(0, 0, 1, 1).data].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

export function browserEnv(): Env {
  return {
    wasm: async (assets) => {
      await loadWasm(assets?.wasm ?? asset(assets, "cavea_web_bg.wasm"));
      return (canvas): EngineView => new View(canvas);
    },
    // A chart `preloadSeatMap` fetched is used once it is in.
    fetchBytes: (url, signal) => shared.get(href(url)) ?? fetchBytes(url, signal),
    atlas,
    mount(element) {
      let shadow: ShadowRoot;
      try {
        shadow = element.shadowRoot ?? element.attachShadow({ mode: "open" });
      } catch (error) {
        const message = "the element cannot hold the map's shadow root";
        throw new CaveaError("invalid_option", message, true, error, "element");
      }
      const doc = element.ownerDocument;
      let style: HTMLStyleElement | undefined;
      try {
        if (sheet === undefined) {
          sheet = new CSSStyleSheet();
          sheet.replaceSync(css());
        }
        shadow.adoptedStyleSheets = [...shadow.adoptedStyleSheets, sheet];
      } catch {
        // No constructable stylesheets (Safari before 16.4), or another document's.
        style = doc.createElement("style");
        style.textContent = css();
        shadow.append(style);
      }
      const root = doc.createElement("div");
      root.className = "r";
      const canvas = doc.createElement("canvas");
      root.append(canvas);
      shadow.append(root);
      return {
        canvas,
        root,
        size: () => [root.clientWidth, root.clientHeight],
        tokens(names) {
          const computed = getComputedStyle(root);
          return names.map((name) => {
            const value = computed.getPropertyValue(`--cavea-${name}`).trim();
            return value !== "" && isColourToken(name) ? hex(value) : value;
          });
        },
        watch(listener) {
          const observer = new MutationObserver(listener);
          for (const node of [element, doc.documentElement, doc.body]) {
            if (node !== null) {
              observer.observe(node, { attributes: true });
            }
          }
          return () => observer.disconnect();
        },
        remove() {
          style?.remove();
          root.remove();
          shadow.adoptedStyleSheets = shadow.adoptedStyleSheets.filter((s) => s !== sheet);
        },
      };
    },
    attach,
    darkScheme(listener) {
      const query = matchMedia("(prefers-color-scheme: dark)");
      const changed = (): void => listener(query.matches);
      changed();
      query.addEventListener("change", changed);
      return () => query.removeEventListener("change", changed);
    },
    online: () => navigator.onLine !== false,
    usable: (signal) =>
      new Promise((resolve) => {
        const check = (): void => {
          if (!document.hidden && navigator.onLine !== false) {
            done();
            resolve();
          }
        };
        const done = (): void => {
          document.removeEventListener("visibilitychange", check);
          removeEventListener("online", check);
          signal.removeEventListener("abort", done);
        };
        document.addEventListener("visibilitychange", check);
        addEventListener("online", check);
        signal.addEventListener("abort", done);
        check();
      }),
    retryMs: 1_000,
    holdTimeoutMs: HOLD_TIMEOUT_MS,
    contextLostMs: 5_000,
    metricsMs: 1_000,
  };
}
