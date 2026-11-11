// The browser `Env` for `createSeatMap`. Nothing here runs at import, so the
// package can be imported where there is no DOM (server-side rendering).
import init, { View, type InitInput } from "../wasm/cavea_web.js";
import { loadGlyphAtlas } from "./atlas.ts";
import { CaveaError } from "./errors.ts";
import { attach } from "./host.ts";
import type { SeatMapOptions } from "./options.ts";
import type { Env } from "./seatmap.ts";
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
    return Promise.reject(new Error("WebAssembly is not available"));
  }
  loading ??= init({
    module_or_path: source ?? new URL("../wasm/cavea_web_bg.wasm", import.meta.url),
  }).catch((error: unknown) => {
    loading = undefined;
    throw error;
  });
  return loading;
}

/** `name` under `assets.baseUrl`, which may be relative to the page. */
function asset(assets: SeatMapOptions["assets"], name: string): URL | undefined {
  const base = assets?.baseUrl;
  return base === undefined ? undefined : new URL(name, new URL(base, document.baseURI));
}

// The page's own rules for the element win over `:host`; `hidden` still hides.
const STYLE =
  ":host{display:block;position:relative}:host([hidden]){display:none}" +
  "canvas{position:absolute;inset:0;width:100%;height:100%;display:block;outline:none}";

export function browserEnv(): Env {
  return {
    wasm: async (assets) => {
      await loadWasm(assets?.wasm ?? asset(assets, "cavea_web_bg.wasm"));
      return (canvas): EngineView => new View(canvas);
    },
    async fetchBytes(url, signal) {
      const response = await fetch(url, { signal });
      if (!response.ok) {
        throw new Error(`${String(url)}: HTTP ${response.status}`);
      }
      return new Uint8Array(await response.arrayBuffer());
    },
    atlas: (assets) => loadGlyphAtlas(asset(assets, "glyphs.png")),
    mount(element) {
      let root: ShadowRoot;
      try {
        root = element.shadowRoot ?? element.attachShadow({ mode: "open" });
      } catch (error) {
        const message = "the element cannot hold the map's shadow root";
        throw new CaveaError("invalid_option", message, true, error, "element");
      }
      const style = document.createElement("style");
      style.textContent = STYLE;
      const canvas = document.createElement("canvas");
      // Until the accessible list view exists (A11Y-001).
      canvas.setAttribute("role", "img");
      canvas.setAttribute("aria-label", "Seat map");
      root.append(style, canvas);
      return {
        canvas,
        size: () => [canvas.clientWidth, canvas.clientHeight],
        remove() {
          style.remove();
          canvas.remove();
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
    holdTimeoutMs: HOLD_TIMEOUT_MS,
    contextLostMs: 5_000,
    metricsMs: 1_000,
  };
}
