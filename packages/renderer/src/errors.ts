// Errors and development warnings (docs/05-web-sdk.md §3, §9).

/** Stable machine codes of `CaveaError` (05 §9). */
export type CaveaErrorCode =
  /** An option is invalid; `option` names it. Rejects `createSeatMap`. */
  | "invalid_option"
  /** `createSeatMap` ran outside a browser. */
  | "not_browser"
  /** WebAssembly is missing or the module failed to fetch or compile. */
  | "wasm_unavailable"
  /** No WebGL2 context could be created. */
  | "webgl_unavailable"
  /** The chart could not be fetched, or no source was given. */
  | "chart_fetch_failed"
  /** The chart bytes are not a compiled chart. */
  | "bad_chart"
  /** The state adapter's `connect` failed. */
  | "state_connect_failed"
  /** The adapter reported an error, or sent state the engine refused. */
  | "state_failed"
  /** The realtime connection dropped (it reconnects) or stopped (`caveaServer`). */
  | "state_disconnected"
  /** The adapter's `hold` rejected or timed out; the hold was reverted. */
  | "hold_failed"
  /** The adapter's `release` rejected or timed out. */
  | "release_failed"
  /** A host callback threw. */
  | "callback_failed"
  /** The glyph atlas failed: seats are drawn without labels. */
  | "glyph_atlas_failed"
  /** The WebGL context was lost and not restored within 5 s; the map redraws if it comes back. */
  | "context_lost"
  /** A method was given input the engine refused (such as unknown labels). */
  | "invalid_input"
  /** The engine reported a failure while drawing or taking input. */
  | "engine_failed"
  /** The host aborted `options.signal` before the map was ready. */
  | "aborted";

export class CaveaError extends Error {
  readonly code: CaveaErrorCode;
  /** Fatal errors reject `createSeatMap`; after that every error is recoverable. */
  readonly fatal: boolean;
  /** The option at fault, for `invalid_option`, as a path such as `selection.max`. */
  readonly option: string | undefined;

  constructor(
    code: CaveaErrorCode,
    message: string,
    fatal = false,
    cause?: unknown,
    option?: string,
  ) {
    super(message, cause === undefined ? undefined : { cause });
    this.name = "CaveaError";
    this.code = code;
    this.fatal = fatal;
    this.option = option;
  }
}

/**
 * Whether a failure cannot succeed if tried again (05 §9): an HTTP status of
 * 400 to 499 other than 408 and 429 (an `Error` with a `status`, as the
 * SDK's fetches throw), or WebAssembly that does not compile or link, or that
 * a Content Security Policy refuses.
 */
export function permanent(error: unknown): boolean {
  const status = (error as { status?: unknown } | null)?.status;
  if (typeof status === "number") {
    return status >= 400 && status < 500 && status !== 408 && status !== 429;
  }
  const name = (error as { name?: unknown } | null)?.name;
  return name === "CompileError" || name === "LinkError" || name === "EvalError";
}

/** An `Error` for an HTTP answer that was not OK, carrying its status. */
export const httpError = (what: string, status: number): Error =>
  Object.assign(new Error(`${what}: HTTP ${status}`), { status });

// Declared here, not taken from @types/node: bundlers replace the expression
// below with a literal, and without one `process` does not exist.
declare const process: { env: { NODE_ENV?: string } };

/**
 * Development builds (05 §3): any build whose bundler does not replace
 * `process.env.NODE_ENV` with `"production"`, including pages with no
 * bundler and no `process` at all. Read at each `createSeatMap`.
 */
export function isDevelopment(): boolean {
  try {
    return process.env.NODE_ENV !== "production";
  } catch {
    return true;
  }
}

/** The one way the package writes to the console: development warnings, each once per map. */
export function warner(on: boolean): (message: string) => void {
  const seen = new Set<string>();
  return (message) => {
    if (on && !seen.has(message)) {
      seen.add(message);
      console.warn(`[cavea] ${message}`);
    }
  };
}
