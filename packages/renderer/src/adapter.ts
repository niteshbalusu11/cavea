// State adapters (docs/05-web-sdk.md §4): where seat state comes from and
// where holds go. The map calls the adapter; the adapter pushes state back
// through its `AdapterContext`.
import type { ImageAsset } from "./images.ts";

/** Where the compiled chart (`.ccf`) comes from. */
export type ChartSource = { url: string | URL } | { bytes: ArrayBuffer | Uint8Array };

/** Seat state as a whole. */
export type Snapshot =
  /** One realtime SNAPSHOT frame of the server's wire format (08 §8), as received. */
  | { kind: "binary"; bytes: ArrayBuffer | Uint8Array }
  /**
   * By label: every object is available except `unavailable`. `mine`: the
   * buyer's own holds, which stay selected (and are selected again after a
   * reload); `expiresAt`: when they expire, Unix ms.
   */
  | { kind: "map"; unavailable: string[]; mine?: string[]; expiresAt?: number };

/** Changes by label since the last snapshot or delta. A label in both lists ends unavailable. */
export interface LabelDelta {
  unavailable?: string[];
  available?: string[];
}

/** One refused object and why: a server code such as `object_not_free`. */
export interface HoldRefusal {
  label: string;
  reason: string;
}

/**
 * The answer to `hold`. Holds are all or nothing: on refusal no object is
 * held. `failed` names the objects at fault, each with its reason; objects it
 * does not name are requested again with the next hold. Without `failed`,
 * every object of the request fails with `code`. `code` `not_found` means the
 * hold token is gone: the whole selection is dropped.
 */
export type HoldResult = { ok: true } | { ok: false; code?: string; failed?: HoldRefusal[] };

/** What the map gives an adapter. Calls made before the map is ready are kept and applied in order. */
export interface AdapterContext {
  /** Aborted when the map is destroyed or fails to load: pass it to `fetch`. */
  readonly signal: AbortSignal;
  /** A SNAPSHOT, DELTA or PRIVATE frame of the wire format (08 §8), as received. */
  applyFrame(bytes: ArrayBuffer | Uint8Array): void;
  /** Replaces all state (after a resync, for example). */
  applySnapshot(snapshot: Snapshot): void;
  /** Label changes; only for adapters that send `map` snapshots. */
  applyDelta(delta: LabelDelta): void;
  /** The buyer's own holds by label, replacing the previous set; `expiresAt` in Unix ms. */
  setMine(labels: string[], expiresAt?: number): void;
  /** The hold token, reported in `getSelection()` and `onSelectionChange`. */
  setHoldToken(token: string | null): void;
  /**
   * The server's clock minus this page's (`Date.now()`), in ms, once known.
   * Expiries the adapter reports (`expiresAt`, PRIVATE frames) are on the
   * server's clock; the map converts them with this.
   */
  setClockOffset(ms: number): void;
  /** Reports a recoverable problem through `onError`. */
  reportError(error: unknown): void;
}

/**
 * A source of seat state. Ordering the map guarantees: `chart` (when the
 * host gave no chart) and `connect` are called once, at start, together;
 * `hold` and `release` are called one at a time in the order the engine
 * requests them, each after the previous call's promise settled (or timed
 * out); `disconnect` is called once, last, and no other method after it.
 */
export interface StateAdapter {
  /** The chart, when the options give none (such as the event's pinned version). */
  chart?(signal: AbortSignal): Promise<ChartSource>;
  /**
   * Where the chart's background image with content hash `hash` is (a URL
   * or the bytes), unless `assets.image` says. Called after the first frame.
   */
  image?(hash: string): ImageAsset | Promise<ImageAsset>;
  /** Resolves with the first state. The map is ready once it is applied. */
  connect(ctx: AdapterContext): Promise<Snapshot>;
  /** Hold-on-select. Without `hold` and `release` the map runs in manual session mode. */
  hold?(labels: string[]): Promise<HoldResult>;
  /** Release these labels, held by this buyer; with `hold`. */
  release?(labels: string[]): Promise<void>;
  /** The engine saw a gap in the deltas: send a new snapshot through the context. */
  resync?(): void;
  /** The page was hidden: an adapter may drop its live connection. */
  pause?(): void;
  /** The page was shown again after `pause`. */
  resume?(): void;
  /** The map is gone: close every connection. Called once, last. */
  disconnect(): void;
}

/** Fixed state: `unavailable` labels, everything else available. No holds. */
export function staticState(options: { unavailable?: string[] } = {}): StateAdapter {
  return {
    connect: () => Promise.resolve({ kind: "map", unavailable: options.unavailable ?? [] }),
    disconnect() {},
  };
}

/** The host's own backend: implement `connect`, and `hold` and `release` for hold-on-select. */
export function customState(adapter: StateAdapter): StateAdapter {
  return adapter;
}
