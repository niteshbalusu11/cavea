// The options object of `createSeatMap` (docs/05-web-sdk.md §3, §5) and its
// validation. Options whose feature is not built yet are left out of the
// types; passed anyway, they are ignored with a development warning.
import type { ChartSource, StateAdapter } from "./adapter.ts";
import { CaveaError } from "./errors.ts";
import type { GestureHandling } from "./host.ts";

export type ThemeName = "light" | "dark" | "high-contrast";
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
  section?: string;
  row?: string;
  seat?: string;
  /** Floor name. */
  floor: string;
  category: { key: string; label: string };
  flags: { accessible: boolean; companion: boolean; restrictedView: boolean; aisle: boolean };
}

export interface Selection {
  objects: SeatObject[];
  holdToken: string | null;
  /** The selection meets `selection.min`. */
  valid: boolean;
  /** When the buyer's holds expire, if known: Unix ms on this page's clock (`Date.now()`), corrected for the server's. */
  expiresAt: number | null;
}

export interface ReadyInfo {
  chart: { name: string; seats: number; objects: number };
  backend: "webgl2";
  /** Milliseconds since `createSeatMap` was called (04 §24). */
  marks: Record<string, number>;
}

export interface SeatMapOptions {
  /** The compiled chart; default: the state adapter's `chart`. */
  chart?: ChartSource;
  /** Default `staticState()`: everything available, no holds. */
  state?: StateAdapter;
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
  assets?: {
    /** Where `cavea_web_bg.wasm` and `glyphs.png` are served; default next to this package. */
    baseUrl?: string | URL;
    /** The WASM itself or its location: bring your own fetch. */
    wasm?: Response | ArrayBuffer | Uint8Array | WebAssembly.Module | string | URL;
  };
  onReady?: (info: ReadyInfo) => void;
  onSelectionChange?: (selection: Selection) => void;
  onObjectSelect?: (object: SeatObject) => void;
  onObjectDeselect?: (object: SeatObject) => void;
  /** `rule` `min`: below the minimum (`objects`: the selection). Otherwise `objects` were refused. */
  onSelectionInvalid?: (rule: string, objects: SeatObject[]) => void;
  onSelectionValid?: () => void;
  onHoldPending?: () => void;
  onHoldSettled?: () => void;
  onHoldFailed?: (failed: Array<{ object: SeatObject; reason: string }>) => void;
  /** The buyer's hold on these selected objects is gone (expired, released elsewhere). */
  onHoldLost?: (objects: SeatObject[]) => void;
  /** 60 s before the buyer's holds expire (or at once, if nearer), with the seconds left. */
  onHoldExpiring?: (secondsLeft: number) => void;
  /** The buyer's holds expired; they have left the selection (after `onHoldLost`). */
  onHoldExpired?: () => void;
  onObjectUnavailable?: (object: SeatObject) => void;
  /** `at`: the object's centre in CSS pixels on the map. */
  onObjectHover?: (object: SeatObject | null, at: [number, number] | null) => void;
  onObjectFocus?: (object: SeatObject | null, at: [number, number] | null) => void;
  onCameraChange?: (camera: { centre: [number, number]; zoom: number; level: string }) => void;
  onError?: (error: CaveaError) => void;
}

/** Options of 05 §3 and callbacks of §5 not built yet: ignored, with a development warning. */
const LATER = new Set(
  (
    "pricing formatPrice floor spotlight zoomToSelect sectionDrillDown ui colorblind locale " +
    "messages dir fallback worker onSectionEnter onFloorChange onMetrics"
  ).split(" "),
);
const SELECTION_LATER = new Set("groupSize validators categories selectable restore".split(" "));
const CALLBACKS =
  "onReady onSelectionChange onObjectSelect onObjectDeselect onSelectionInvalid onSelectionValid " +
  "onHoldPending onHoldSettled onHoldFailed onHoldLost onHoldExpiring onHoldExpired " +
  "onObjectUnavailable onObjectHover onObjectFocus " +
  "onCameraChange onError";

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
  assets: (v) =>
    isObject(v) &&
    (v["baseUrl"] === undefined || isUrl(v["baseUrl"])) &&
    (v["wasm"] === undefined ||
      isUrl(v["wasm"]) ||
      isBytes(v["wasm"]) ||
      v["wasm"] instanceof Response ||
      v["wasm"] instanceof WebAssembly.Module),
};
for (const name of CALLBACKS.split(" ")) {
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
