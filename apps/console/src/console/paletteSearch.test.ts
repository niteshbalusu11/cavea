// The palette's search: what it asks the server, that it waits for typing
// to pause, and that a newer query cancels the older one's request and
// only the newest answer is shown.
import { expect, test } from "bun:test";
import { eventsQuery, Search, type SearchState, type Timers } from "./paletteSearch.ts";

test("the events query is what was typed, trimmed and bounded, across active and archived", () => {
  expect(eventsQuery("  ")).toBeNull();
  expect(eventsQuery(" Opening night ")).toEqual({
    q: "Opening night",
    limit: 8,
    order: "newest",
  });
  // Characters, not UTF-16 units: an emoji is not cut in half.
  const long = "🎭".repeat(200);
  expect([...(eventsQuery(long)?.q ?? "")].length).toBe(128);
  // Sent as typed: `%` and `_` are the server's to treat literally.
  expect(eventsQuery("50%_")?.q).toBe("50%_");
});

/** Timers run by hand. */
function manual(): Timers & { fire(): void; pending(): number } {
  let next: (() => void) | null = null;
  return {
    set: (run) => {
      next = run;
      return run;
    },
    clear: (h) => {
      if (next === h) next = null;
    },
    fire: () => {
      const run = next;
      next = null;
      run?.();
    },
    pending: () => (next === null ? 0 : 1),
  };
}

test("waits for typing to pause, cancels what a newer query replaces, reports only the newest", async () => {
  const timers = manual();
  const asked: { query: string; signal: AbortSignal; answer: (v: string) => void }[] = [];
  const states: SearchState<string>[] = [];
  const search = new Search<string>(
    (query, signal) => new Promise((resolve) => asked.push({ query, signal, answer: resolve })),
    (s) => states.push(s),
    250,
    timers,
  );
  search.set("o");
  search.set("op");
  // Typing restarts the wait: one timer, nothing asked yet.
  expect(timers.pending()).toBe(1);
  expect(asked).toEqual([]);
  timers.fire();
  expect(asked.map((a) => a.query)).toEqual(["op"]);
  // A newer query aborts the request on its way, whose late answer is dropped.
  search.set("ope");
  expect(asked[0]?.signal.aborted).toBe(true);
  asked[0]?.answer("stale");
  timers.fire();
  asked[1]?.answer("fresh");
  await Promise.resolve();
  await Promise.resolve();
  expect(states.at(-1)).toEqual({ kind: "done", query: "ope", value: "fresh" });
  expect(states.some((s) => s.kind === "done" && s.value === "stale")).toBe(false);
  // Cleared: idle, and nothing more is asked.
  search.set("");
  expect(states.at(-1)).toEqual({ kind: "idle" });
  expect(timers.pending()).toBe(0);
});

test("a failure is reported for its own query; dispose cancels the request on its way", async () => {
  const timers = manual();
  const signals: AbortSignal[] = [];
  const states: SearchState<string>[] = [];
  const search = new Search<string>(
    (_q, signal) => {
      signals.push(signal);
      return Promise.reject(new Error("offline"));
    },
    (s) => states.push(s),
    250,
    timers,
  );
  search.set("x");
  timers.fire();
  await Promise.resolve();
  await Promise.resolve();
  expect(states.at(-1)?.kind).toBe("failed");
  const hanging = new Search<string>(
    (_q, signal) => {
      signals.push(signal);
      return new Promise(() => {});
    },
    () => {},
    250,
    timers,
  );
  hanging.set("y");
  timers.fire();
  hanging.dispose();
  expect(signals.at(-1)?.aborted).toBe(true);
});
