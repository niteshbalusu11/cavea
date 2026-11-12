// The options object of `createSeatMap` (docs/05-web-sdk.md §3, §5) and its
// validation. Options whose feature is not built yet are left out of the
// types; passed anyway, they are ignored with a development warning.
import type { ChartSource, StateAdapter } from "./adapter.ts";
import { CaveaError } from "./errors.ts";
import type { Lod, ThemeName } from "./generated/engine.ts";
import type { GestureHandling } from "./host.ts";
import type { ImageSource } from "./images.ts";
import { isMessage, MESSAGES, type Messages } from "./messages.ts";
import type { CategoryPricing, FormatPrice } from "./pricing.ts";
import type { UiOptions } from "./ui.ts";

export type { ThemeName };

/** A built-in theme, `auto` (light or dark, following the system), or tokens over one (05 §8). */
export type Theme =
  | ThemeName
  | "auto"
  | { base?: ThemeName | "auto"; tokens: Record<string, string | number> };

/** A rectangle in chart units. */
export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** One object, as callbacks and methods report it (05 §5). */
export interface SeatObject {
  /** Full label, for example `"Orchestra-C-12"`. */
  label: string;
  /** Only seats are selectable today. */
  kind: "seat" | "table" | "booth" | "area";
  /** Section label; left out when the chart has no sections. */
  section?: string;
  /** Row label; left out when the object has none. */
  row?: string;
  /** Seat label within the row; left out when the object has none. */
  seat?: string;
  /** Floor name. */
  floor: string;
  /** The object's category: its key in the chart and its name. */
  category: { key: string; label: string };
  /** The ticket type chosen in the tooltip, for a selected object whose category has types (default the first). */
  ticketType?: string;
  /** The seat's flags, as set in the designer. */
  flags: { accessible: boolean; companion: boolean; restrictedView: boolean; aisle: boolean };
  /** From `pricing`, in minor units: the ticket type's price, else the category's. */
  price?: number;
}

export interface Selection {
  /** The selected objects, in the order they were selected. */
  objects: SeatObject[];
  /** The hold token holding them, to book with from the host's backend; `null` while there is none. */
  holdToken: string | null;
  /** The selection meets `selection.min`. */
  valid: boolean;
  /** When the buyer's holds expire, if known: Unix ms on this page's clock (`Date.now()`), corrected for the server's. */
  expiresAt: number | null;
}

export interface ReadyInfo {
  /** The chart's name, its seats, and its bookable objects (seats, and later tables, booths and areas). */
  chart: { name: string; seats: number; objects: number };
  /** What draws the map: WebGL2, the only backend today. */
  backend: "webgl2";
  /** Milliseconds since `createSeatMap` was called (04 §24). */
  marks: Record<string, number>;
}

export interface SeatMapOptions {
  /** The compiled chart; default: the state adapter's `chart`. */
  chart?: ChartSource;
  /** Default `staticState()`: everything available, no holds. */
  state?: StateAdapter;
  /** Prices for display, in minor units (05 §3.1). */
  pricing?: CategoryPricing[];
  /** Default: `Intl.NumberFormat` for `locale`, in `currency` if given. */
  formatPrice?: FormatPrice;
  /** ISO 4217 code for the default price format, such as `"GBP"`; its minor unit divides prices. */
  currency?: string;
  selection?: {
    /** 1 to 100; default 10. */
    max?: number;
    /** Checkout is not `valid` below this; default 0. */
    min?: number;
  };
  /** Only `picker` exists yet. */
  mode?: "picker";
  /** Default `greedy`. */
  gestureHandling?: GestureHandling;
  /** `fit` (default), a section label, or bounds. */
  initialView?: "fit" | string | Bounds;
  /** Default 2 on touch devices, 3 otherwise. */
  maxDpr?: number;
  /** Default `auto`: follow `prefers-reduced-motion`. */
  reducedMotion?: "auto" | boolean;
  /** Default `auto`. */
  theme?: Theme;
  /** The built-in interface; each element is on unless `false` (05 §3.4, §7). */
  ui?: UiOptions;
  /** BCP 47 tag for messages, numbers and prices; default the browser's language. */
  locale?: string;
  /** Overrides of the English message catalogue, by id (05 §3.4). */
  messages?: Messages;
  /** The interface's direction; `auto` (default): `locale`'s if given, else the page's. */
  dir?: "ltr" | "rtl" | "auto";
  /**
   * The licence file's text, for a map without `caveaServer` (12 §7.4):
   * verified in the browser after the first interactive frame. Without it,
   * or when it does not verify, the map carries a "Development licence"
   * mark; it never stops the map. Ignored with `caveaServer`, whose server
   * says.
   */
  licence?: string;
  /**
   * Aborting it before the map is ready rejects `createSeatMap` (`aborted`)
   * and removes everything; after, it destroys the map (05 §9).
   */
  signal?: AbortSignal;
  assets?: {
    /** Where `cavea_web_bg.wasm` and `glyphs.png` are served; default next to this package. */
    baseUrl?: string | URL;
    /** The WASM itself or its location: bring your own fetch. */
    wasm?: Response | ArrayBuffer | Uint8Array | WebAssembly.Module | string | URL;
    /**
     * Where the chart's background images are, by content hash: a URL to
     * fetch or the bytes. Default: the state adapter's `image`, if any.
     */
    image?: ImageSource;
  };
  /** The chart is decoded and the first state applied (a hidden container has not drawn yet); just before `createSeatMap` resolves. */
  onReady?: (info: ReadyInfo) => void;
  /** The selection changed, or the buyer chose another ticket type. */
  onSelectionChange?: (selection: Selection) => void;
  /** One object was added to the selection. */
  onObjectSelect?: (object: SeatObject) => void;
  /** One object left the selection. */
  onObjectDeselect?: (object: SeatObject) => void;
  /** `rule` `min`: below the minimum (`objects`: the selection). Otherwise `objects` were refused. */
  onSelectionInvalid?: (rule: string, objects: SeatObject[]) => void;
  /** The selection meets `selection.min` again. */
  onSelectionValid?: () => void;
  /** The first of a run of hold requests started: disable checkout until `onHoldSettled`. */
  onHoldPending?: () => void;
  /** The last hold request of a run was answered. */
  onHoldSettled?: () => void;
  /** A hold was refused: each object with its reason, such as `not_free`, or the request's code (`hold_limit_exceeded`, `hold_failed`). */
  onHoldFailed?: (failed: Array<{ object: SeatObject; reason: string }>) => void;
  /** The buyer's hold on these selected objects is gone (expired, released elsewhere). */
  onHoldLost?: (objects: SeatObject[]) => void;
  /** 60 s before the buyer's holds expire (or at once, if nearer), with the seconds left. */
  onHoldExpiring?: (secondsLeft: number) => void;
  /** The buyer's holds expired; they have left the selection (after `onHoldLost`). */
  onHoldExpired?: () => void;
  /** A selected object was taken by someone else; it has left the selection. */
  onObjectUnavailable?: (object: SeatObject) => void;
  /** `at`: the object's centre in CSS pixels on the map. */
  onObjectHover?: (object: SeatObject | null, at: [number, number] | null) => void;
  /** The keyboard focus moved to an object, or was hidden (`null`); `at` as `onObjectHover`'s. */
  onObjectFocus?: (object: SeatObject | null, at: [number, number] | null) => void;
  /** The camera moved (at most once a frame, throughout a pan, a zoom or an animation) or the level of detail changed: the chart point at the centre, the zoom in CSS pixels per chart unit, the level. */
  onCameraChange?: (camera: { centre: [number, number]; zoom: number; level: string }) => void;
  /** Frame statistics and timing marks, at most once a second while frames are drawn (04 §24). */
  onMetrics?: (metrics: Metrics) => void;
  /** Any error after `createSeatMap` resolved, and a recoverable one before. Given, no development warning is printed for errors. */
  onError?: (error: CaveaError) => void;
}

/** `onMetrics`' payload (05 §5, 04 §24). */
export interface Metrics {
  /** Frames timed: the last 120 at most. */
  frames: number;
  /** The page's work for a frame in ms (the map's frame, text drawing and its callbacks; not GPU time). */
  frameMs: { median: number; p95: number; max: number };
  /** In the last frame drawn. */
  drawCalls: number;
  /** Seats in the ranges the last frame drew. */
  visibleSeats: number;
  /** The level of detail of the last frame drawn: `overview`, `crossFade`, `seats` or `detail`. */
  level: Lod | null;
  /** What draws the map: WebGL2, the only backend today. */
  backend: "webgl2";
  /** The device pixel ratio rendered at. */
  pixelRatio: number;
  /** Timing marks in ms since `createSeatMap`, as `onReady`'s, with `interactive` once drawn. */
  marks: Record<string, number>;
}

/** Options of 05 §3 and callbacks of §5 not built yet: ignored, with a development warning. */
const LATER = new Set(
  (
    "floor spotlight zoomToSelect sectionDrillDown colorblind fallback worker onSectionEnter " +
    "onFloorChange"
  ).split(" "),
);
const UI_LATER = new Set("floorSelector filters holdTimer listViewToggle".split(" "));
const SELECTION_LATER = new Set("groupSize validators categories selectable restore".split(" "));
const CALLBACKS =
  "onReady onSelectionChange onObjectSelect onObjectDeselect onSelectionInvalid onSelectionValid " +
  "onHoldPending onHoldSettled onHoldFailed onHoldLost onHoldExpiring onHoldExpired " +
  "onObjectUnavailable onObjectHover onObjectFocus " +
  "onCameraChange onMetrics onError";

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;
const isUrl = (v: unknown): boolean => typeof v === "string" || v instanceof URL;
const isBytes = (v: unknown): boolean => v instanceof ArrayBuffer || v instanceof Uint8Array;
const isInt = (v: unknown, min: number, max: number): boolean =>
  Number.isInteger(v) && (v as number) >= min && (v as number) <= max;
const finite = (v: unknown): boolean => typeof v === "number" && Number.isFinite(v);
export const isBounds = (v: unknown): v is Bounds =>
  isObject(v) &&
  finite(v["x"]) &&
  finite(v["y"]) &&
  (v["width"] as number) > 0 &&
  (v["height"] as number) > 0 &&
  finite(v["width"]) &&
  finite(v["height"]);
/** Whether `Intl` takes `v` as a locale (and `currency` as a currency). */
export const isLocale = (v: unknown, currency?: unknown): boolean => {
  if (typeof v !== "string" || (currency !== undefined && typeof currency !== "string")) {
    return false;
  }
  try {
    const options: Intl.NumberFormatOptions =
      currency === undefined ? {} : { style: "currency", currency };
    return new Intl.NumberFormat(v, options).resolvedOptions().locale !== "";
  } catch {
    return false;
  }
};
const THEMES = ["light", "dark", "high-contrast", "auto"];
export const isTheme = (v: unknown): v is Theme =>
  THEMES.includes(v as string) ||
  (isObject(v) &&
    (v["base"] === undefined || THEMES.includes(v["base"] as string)) &&
    isObject(v["tokens"]) &&
    Object.values(v["tokens"]).every((t) => typeof t === "string" || finite(t)));

const CHECKS: Record<string, (v: unknown) => boolean> = {
  chart: (v) => isObject(v) && (isUrl(v["url"]) || isBytes(v["bytes"])),
  state: (v) =>
    isObject(v) &&
    typeof v["connect"] === "function" &&
    typeof v["disconnect"] === "function" &&
    // Both or neither: a hold the map cannot release would stay held.
    typeof v["hold"] === typeof v["release"] &&
    ["undefined", "function"].includes(typeof v["hold"]),
  selection: isObject,
  mode: (v) => typeof v === "string",
  gestureHandling: (v) => ["greedy", "cooperative", "none"].includes(v as string),
  initialView: (v) => typeof v === "string" || isBounds(v),
  maxDpr: (v) => finite(v) && (v as number) > 0,
  reducedMotion: (v) => v === "auto" || typeof v === "boolean",
  theme: isTheme,
  pricing: Array.isArray,
  currency: (v) => isLocale("en", v),
  locale: (v) => isLocale(v),
  messages: (v) => isObject(v) && Object.values(v).every((m) => m === undefined || isMessage(m)),
  dir: (v) => ["ltr", "rtl", "auto"].includes(v as string),
  // Any value: one that is not a licence shows the mark, never refuses the map (D-20).
  licence: () => true,
  ui: isObject,
  signal: (v) => isObject(v) && typeof v["aborted"] === "boolean" && "addEventListener" in v,
  assets: (v) =>
    isObject(v) &&
    (v["baseUrl"] === undefined || isUrl(v["baseUrl"])) &&
    (v["image"] === undefined || typeof v["image"] === "function") &&
    (v["wasm"] === undefined ||
      isUrl(v["wasm"]) ||
      isBytes(v["wasm"]) ||
      v["wasm"] instanceof Response ||
      v["wasm"] instanceof WebAssembly.Module),
};
for (const name of `${CALLBACKS} formatPrice`.split(" ")) {
  CHECKS[name] = (v) => typeof v === "function";
}

function invalid(option: string, value: unknown): CaveaError {
  return new CaveaError(
    "invalid_option",
    `option "${option}" is invalid: ${String(value)}`,
    true,
    undefined,
    option,
  );
}

/**
 * Checks `options` against 05 §3: rejects an invalid value with a
 * `CaveaError` naming the option; warns (in development) about unknown keys
 * and options that are not supported yet, which are ignored.
 */
export function validateOptions(options: unknown, warn: (message: string) => void): SeatMapOptions {
  if (options === undefined) {
    return {};
  }
  if (!isObject(options)) {
    throw invalid("options", options);
  }
  for (const [key, value] of Object.entries(options)) {
    if (value === undefined) {
      continue;
    }
    const check = CHECKS[key];
    if (check === undefined) {
      warn(
        LATER.has(key)
          ? `option "${key}" is not supported yet and is ignored`
          : `unknown option "${key}" is ignored`,
      );
    } else if (!check(value)) {
      throw invalid(key, value);
    }
  }
  const ui = options["ui"];
  if (isObject(ui)) {
    for (const [key, value] of Object.entries(ui)) {
      if (
        UI_LATER.has(key) ||
        !["zoomControls", "legend", "tooltip", "toasts", "loading"].includes(key)
      ) {
        warn(
          UI_LATER.has(key)
            ? `option "ui.${key}" is not supported yet and is ignored`
            : `unknown option "ui.${key}" is ignored`,
        );
      } else if (
        value !== undefined &&
        typeof value !== "boolean" &&
        !(key === "tooltip" && typeof value === "function")
      ) {
        throw invalid(`ui.${key}`, value);
      }
    }
  }
  for (const id of Object.keys(isObject(options["messages"]) ? options["messages"] : {})) {
    if (!Object.keys(MESSAGES).includes(id)) {
      warn(`unknown message "${id}" is ignored`);
    }
  }
  if (options["mode"] !== undefined && options["mode"] !== "picker") {
    warn(`mode "${String(options["mode"])}" is not supported yet; the map runs in picker mode`);
  }
  const selection = options["selection"];
  if (isObject(selection)) {
    const { max = 10, min = 0 } = selection;
    for (const key of Object.keys(selection)) {
      if (key !== "max" && key !== "min") {
        warn(
          SELECTION_LATER.has(key)
            ? `option "selection.${key}" is not supported yet and is ignored`
            : `unknown option "selection.${key}" is ignored`,
        );
      }
    }
    if (!isInt(max, 1, 100)) {
      throw invalid("selection.max", max);
    }
    if (!isInt(min, 0, max as number)) {
      throw invalid("selection.min", min);
    }
  }
  return options as SeatMapOptions;
}
