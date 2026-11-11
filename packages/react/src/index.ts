// `@cavea/react`: the seat map as one React component (docs/05-web-sdk.md
// §11). Every option of `createSeatMap` is a prop; the ref is the map. The
// map is created after mount and destroyed at unmount; rendering (on the
// server too) only makes the container, so nothing here touches the DOM
// before an effect runs.
import {
  createSeatMap,
  type CaveaError,
  type SeatMap as SeatMapInstance,
  type SeatMapOptions,
} from "@cavea/renderer";
import {
  createElement,
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ForwardedRef,
  type ReactElement,
} from "react";

/**
 * The options of `createSeatMap`, less `signal` (the component's mount is
 * the map's lifetime), with the container's `id`, `className` and `style`.
 */
export interface SeatMapProps extends Omit<SeatMapOptions, "signal"> {
  id?: string;
  className?: string;
  style?: CSSProperties;
}

type Options = Omit<SeatMapOptions, "signal">;
type Key = keyof Options;

/** Props a live map takes through a method instead of being created again (05 §6). */
const SETTERS = {
  pricing: (map: SeatMapInstance, value: Options["pricing"]) => map.setPricing(value ?? []),
  theme: (map: SeatMapInstance, value: Options["theme"]) => map.setTheme(value ?? "auto"),
  locale: (map: SeatMapInstance, value: Options["locale"]) =>
    map.setLocale(value ?? globalThis.navigator?.language ?? "en"),
} as const;
type Setter = keyof typeof SETTERS;

/**
 * Callbacks passed as stable functions that call the latest prop, so a new
 * function each render neither recreates the map nor resubscribes. Not these
 * three, whose presence changes what the map does (`onMetrics` times frames,
 * `formatPrice` replaces the default format, `onError` silences development
 * warnings): they are passed only if given at creation.
 */
const PRESENCE = new Set<Key>(["onMetrics", "onError", "formatPrice"]);
const isCallback = (key: string): boolean => key.startsWith("on") || key === "formatPrice";

/** Props that only matter at creation, so a change is not worth a warning. */
const INITIAL = new Set<Key>(["initialView"]);

// Bundlers replace this expression; a page without one has no `process`.
declare const process: { env: { NODE_ENV?: string } };
const development = (): boolean => {
  try {
    return process.env.NODE_ENV !== "production";
  } catch {
    return true;
  }
};
const warn = (message: string): void => {
  if (development()) {
    console.warn(`[cavea/react] ${message}`);
  }
};

const plain = (v: object): boolean => Object.getPrototypeOf(v) === Object.prototype;

/**
 * Equal as data: primitives, arrays and plain objects by value, functions
 * always (they are called through the latest props), anything else by identity.
 */
function same(a: unknown, b: unknown): boolean {
  if (Object.is(a, b) || (typeof a === "function" && typeof b === "function")) {
    return true;
  }
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) {
    return false;
  }
  if (Array.isArray(a)) {
    return Array.isArray(b) && a.length === b.length && a.every((v, i) => same(v, b[i]));
  }
  if (!plain(a) || !plain(b)) {
    return false;
  }
  const keys = Object.keys(a);
  return (
    keys.length === Object.keys(b).length &&
    keys.every((k) => same((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]))
  );
}

/** What identifies the chart: its URL as text, or its bytes by identity. */
const chartKey = (chart: Options["chart"]): unknown =>
  chart === undefined ? undefined : "url" in chart ? String(chart.url) : chart.bytes;

// `useLayoutEffect` warns on the server in React 18; nothing runs there anyway.
const useClientLayoutEffect = typeof document === "undefined" ? useEffect : useLayoutEffect;

function SeatMapComponent(props: SeatMapProps, ref: ForwardedRef<SeatMapInstance>): ReactElement {
  const { id, className, style, ...options } = props;
  const container = useRef<HTMLDivElement>(null);
  const latest = useRef<Options>(options);
  /** The options the live map was created with or last given, by prop. */
  const applied = useRef<Options>({});
  const recreated = useRef({ state: options.state, chart: chartKey(options.chart), runs: 0 });
  const [map, setMap] = useState<SeatMapInstance | null>(null);
  // `null` until the map is ready, as a ref to an element is until it mounts.
  useImperativeHandle(ref, () => map as SeatMapInstance, [map]);

  useClientLayoutEffect(() => {
    latest.current = options;
  });

  const key = chartKey(options.chart);
  useEffect(() => {
    const element = container.current;
    if (element === null) {
      return undefined;
    }
    const controller = new AbortController();
    // A microtask later: Strict Mode mounts, unmounts and mounts again in one
    // go, so its first mount is cancelled before it fetches or connects
    // anything, and a state adapter (which serves one map) is connected once.
    queueMicrotask(() => {
      if (controller.signal.aborted) {
        return;
      }
      const given = latest.current;
      const created: SeatMapOptions = { signal: controller.signal };
      for (const name of Object.keys(given) as Key[]) {
        const value = given[name];
        if (value === undefined) {
          continue;
        }
        Object.assign(created, {
          [name]: isCallback(name)
            ? (...args: unknown[]) =>
                (latest.current[name] as ((...a: unknown[]) => unknown) | undefined)?.(...args)
            : value,
        });
      }
      // A tooltip function, too, is the latest prop's.
      if (typeof given.ui?.tooltip === "function") {
        created.ui = {
          ...given.ui,
          tooltip: (object) => {
            const tooltip = latest.current.ui?.tooltip;
            return typeof tooltip === "function" ? tooltip(object) : null;
          },
        };
      }
      applied.current = { ...given };
      createSeatMap(element, created).then(
        (ready) => {
          if (!controller.signal.aborted) {
            setMap(ready);
          }
        },
        (error: CaveaError) => {
          if (controller.signal.aborted) {
            return;
          }
          const onError = latest.current.onError;
          if (onError === undefined) {
            throw error;
          }
          onError(error);
        },
      );
    });
    return () => {
      // Aborting destroys a map that is ready and cancels one that is loading (05 §9).
      controller.abort();
      setMap(null);
    };
  }, [options.state, key]);

  // After each render: setters for what changed, warnings for what cannot.
  useEffect(() => {
    const was = recreated.current;
    const again = was.state !== options.state || was.chart !== key;
    recreated.current = { state: options.state, chart: key, runs: again ? was.runs + 1 : 0 };
    if (again && was.runs + 1 === 3) {
      warn(
        "the map was recreated on three renders in a row: `state` (or `chart`) is a new object " +
          "each render. Create the adapter once, with `useMemo` or outside the component",
      );
    }
    // A new map is on its way (this render's `map` is the old one, being destroyed).
    if (map === null || again) {
      return;
    }
    const before = applied.current;
    const keys = new Set([...Object.keys(before), ...Object.keys(options)] as Key[]);
    for (const name of keys) {
      const value = options[name];
      if (name === "state" || name === "chart" || INITIAL.has(name)) {
        continue;
      }
      if (isCallback(name) && !PRESENCE.has(name)) {
        continue;
      }
      if (
        isCallback(name)
          ? (value === undefined) === (before[name] === undefined)
          : same(value, before[name])
      ) {
        continue;
      }
      if (name in SETTERS) {
        const setter = SETTERS[name as Setter] as (m: SeatMapInstance, v: unknown) => void;
        setter(map, value);
      } else {
        warn(
          `prop "${name}" changed after the map was created and has no setter: ` +
            "it applies when the map is created again (a new `state` or `chart`)",
        );
      }
      Object.assign(before, { [name]: value });
    }
  });

  return createElement("div", { ref: container, id, className, style });
}

/**
 * The seat map. Props are the options of `createSeatMap` (05 §3, §5);
 * `pricing`, `theme` and `locale` are applied to the live map when they
 * change, and a new `state` adapter or chart creates the map again. The ref
 * is the map (05 §6) once it is ready, `null` before and after.
 */
export const SeatMap = forwardRef<SeatMapInstance, SeatMapProps>(SeatMapComponent);
SeatMap.displayName = "SeatMap";

export type { SeatMapInstance };
