// The console's command palette's search (09 §8: Ctrl+K "also searches
// charts and events"), as plain logic: which keys open the palette and the
// help, what is asked of the server, and a search that waits for typing to
// pause and cancels the request it replaces.
import { focusOf } from "../designer/keys.ts";
import { matches, SHORTCUTS, type Press } from "../designer/shortcuts.ts";

/** The most a search sends: the events list's own limit (08 §6.2). */
export const MAX_QUERY = 128;
/** Typing pauses this long before a search is sent. */
export const DEBOUNCE_MS = 250;
/** Results of each kind shown. */
export const RESULTS = 8;

/** The events list's query for what was typed, or `null` for nothing to
 * search: trimmed, at most `MAX_QUERY` characters, newest first; no
 * `archived`, so active and archived events alike (the palette finds what
 * a person remembers). */
export function eventsQuery(typed: string): { q: string; limit: number; order: "newest" } | null {
  const q = [...typed.trim()].slice(0, MAX_QUERY).join("");
  return q === "" ? null : { q, limit: RESULTS, order: "newest" };
}

/** What the console frame does with a key press: the designer's own two
 * chords (`shortcuts.ts`), one table for both. Never while a text field
 * has focus (the person is typing), nor inside a dialog or menu, nor while
 * the designer's own window keys are bound (it has its own palette). */
export function consoleKey(
  press: Press & { target: EventTarget | null; defaultPrevented: boolean; isComposing: boolean },
  mac: boolean,
): "palette" | "help" | null {
  if (press.defaultPrevented || press.isComposing) return null;
  const t = press.target;
  if (
    t instanceof Element &&
    t.closest("[role=dialog],[role=alertdialog],[role=menu],[role=listbox]") !== null
  ) {
    return null;
  }
  if (focusOf(t) === "text") return null;
  for (const id of ["palette", "help"] as const) {
    const s = SHORTCUTS.find((x) => x.id === id);
    if (s?.chords.some((c) => matches(c, press, mac)) === true) return id;
  }
  return null;
}

export type SearchState<T> =
  | { kind: "idle" }
  /** Typed; waiting for the pause, or for the server. */
  | { kind: "searching"; query: string }
  | { kind: "done"; query: string; value: T }
  | { kind: "failed"; query: string; error: unknown };

export interface Timers {
  set(run: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

const realTimers: Timers = {
  set: (run, ms) => setTimeout(run, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

/**
 * A search as the person types: each change restarts the wait; once typing
 * pauses for `delay`, `run` is called with a signal that a later change, a
 * clear or `dispose` aborts. Only the latest query's answer is reported;
 * an aborted request reports nothing.
 */
export class Search<T> {
  #timer: unknown = null;
  #request: AbortController | null = null;
  #query = "";

  constructor(
    private readonly run: (query: string, signal: AbortSignal) => Promise<T>,
    private readonly report: (state: SearchState<T>) => void,
    private readonly delay = DEBOUNCE_MS,
    private readonly timers: Timers = realTimers,
  ) {}

  set(query: string): void {
    if (query === this.#query) return;
    this.#query = query;
    this.#stop();
    if (query === "") return this.report({ kind: "idle" });
    this.report({ kind: "searching", query });
    this.#timer = this.timers.set(() => {
      this.#timer = null;
      const request = new AbortController();
      this.#request = request;
      this.run(query, request.signal).then(
        (value) => {
          if (this.#request !== request) return;
          this.#request = null;
          this.report({ kind: "done", query, value });
        },
        (error: unknown) => {
          if (this.#request !== request) return;
          this.#request = null;
          this.report({ kind: "failed", query, error });
        },
      );
    }, this.delay);
  }

  dispose(): void {
    this.#query = "";
    this.#stop();
  }

  #stop(): void {
    if (this.#timer !== null) this.timers.clear(this.#timer);
    this.#timer = null;
    this.#request?.abort();
    this.#request = null;
  }
}
