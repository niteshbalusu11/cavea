// The event manager's view of the seats (09 §4.2, CON-008; D-68), kept
// current by polling the staff routes, since the console has no stream it
// may open: the public stream takes a publishable key, whose value the
// console cannot read back, and the manager stream with domain statuses is
// not built (SRV-039). Every `TICK_MS` it reads the event's counts and,
// by label, the seats the operator is looking at (the selection and the
// seat inspected); it reads every seat that is not free when the counts
// change, at least every `WHOLE_EVERY_MS`, and never more often than its
// last read's cost allows (`wholeReadDue`). It feeds the renderer as a
// `customState` adapter: the map draws a seat unavailable when it is not
// free and not selected, so the picker lets the operator select any seat.
// Nothing runs while the page is hidden; everything stops with `signal`.
import type { AdapterContext, StateAdapter } from "@cavea/renderer";
import { createSignal } from "solid-js";
import {
  checkEvent,
  checkObjectPage,
  checkObjectsByLabel,
  checkSeatReply,
} from "../../api/check.ts";
import { call, isAbort, type ApiError } from "../../api/client.ts";
import type { Counts, Event, EventObject } from "../../api/generated.ts";
import { asApiError } from "../../api/query.ts";
import {
  deltaFor,
  mergeRead,
  refusedSeats,
  sameCounts,
  statusesToRead,
  TICK_MS,
  wholeReadDue,
  type Action,
  type Refusal,
  type Status,
} from "./model.ts";

/** One whole read, for the measurements (dev builds expose them). */
export interface WholeRead {
  requests: number;
  objects: number;
  tookMs: number;
  endedAt: number;
}

export interface Health {
  checkedAt: number | null;
  error: ApiError | null;
}

export type Outcome =
  | { ok: true; labels: string[] }
  | { ok: false; error: ApiError; refused: Refusal[] };

export interface Live {
  /** The map's state adapter. */
  adapter: StateAdapter;
  /** A seat's status as last read: `free` unless known otherwise. */
  statusOf(label: string): Status;
  /** A seat that is not free, with what the server gave (order, expiry). */
  object(label: string): EventObject | undefined;
  /** Changes whenever a status known to the page changes. */
  version(): number;
  /** When the counts and the watched seats were last read, and the last
   * failure if the latest read failed (what is shown may then be stale). */
  health(): Health;
  /** The map's selection: these seats are drawn available whatever their
   * status, so that the picker keeps them; the others go back. */
  setSelected(labels: readonly string[]): void;
  /** Draws `labels` available now, so that the picker takes them when told
   * to select them; `settle` then puts back any it did not take. */
  allow(labels: readonly string[]): void;
  settle(labels: readonly string[]): void;
  inspect(label: string | null): void;
  /** Reads `labels` now (a seat just clicked). */
  read(labels: readonly string[]): Promise<void>;
  /** Acts on `labels`, all or nothing, and reads them again either way. */
  act(action: Action, labels: readonly string[], orderRef?: string): Promise<Outcome>;
  /** The whole reads so far (the last 20), for measurement. */
  reads(): readonly WholeRead[];
}

/** Labels per read by label: the server takes at most 1,000. */
const LABELS_PER_READ = 500;

export function createLive(options: {
  event: string;
  signal: AbortSignal;
  /** Each fresh read of the event (its counts, settings, archive). */
  onEvent: (event: Event) => void;
}): Live {
  const { event, signal } = options;
  let nonFree = new Map<string, EventObject>();
  /** When each seat was last read or changed one by one. */
  const learnt = new Map<string, number>();
  const drawn = new Set<string>();
  let selected = new Set<string>();
  let inspected: string | null = null;
  let ctx: AdapterContext | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let paused = false;
  let stopped = false;
  let reading = false;
  let last: WholeRead | null = null;
  let countsAtRead: Counts | null = null;
  const history: WholeRead[] = [];
  const [version, setVersion] = createSignal(0);
  const [health, setHealth] = createSignal<Health>({ checkedAt: null, error: null });
  const bump = () => setVersion((v) => v + 1);
  /** The last whole read's failure, until one succeeds: the map may be
   * stale while the counts and the selection are read fine. */
  let wholeFailed: ApiError | null = null;

  const hidden = (label: string) => nonFree.has(label) && !selected.has(label);
  /** Tells the map about `labels` (or everything it may draw wrongly). */
  const sync = (labels: Iterable<string>) => {
    const delta = deltaFor(labels, drawn, hidden);
    if (ctx !== null && (delta.unavailable.length > 0 || delta.available.length > 0)) {
      ctx.applyDelta(delta);
    }
  };

  const learn = (objects: readonly EventObject[], labels: readonly string[], at: number) => {
    const given = new Map(objects.map((o) => [o.label, o]));
    for (const label of labels) {
      const o = given.get(label);
      if (o === undefined || o.status === "free") nonFree.delete(label);
      else nonFree.set(label, o);
      learnt.set(label, at);
    }
  };

  async function read(labels: readonly string[]): Promise<void> {
    const unique = [...new Set(labels)];
    for (let i = 0; i < unique.length; i += LABELS_PER_READ) {
      const chunk = unique.slice(i, i + LABELS_PER_READ);
      const at = Date.now();
      const answer = checkObjectsByLabel(
        await call("listObjects", { path: { event }, query: { label: chunk } }, signal),
      );
      // Unknown labels are not in the chart: nothing to learn about them.
      const known = chunk.filter((l) => !answer.unknown.includes(l));
      learn(answer.data, known, at);
      sync(known);
    }
    bump();
  }

  /** Every seat that is not free, a status at a time, in pages. */
  async function whole(counts: Counts): Promise<void> {
    reading = true;
    const started = Date.now();
    let requests = 0;
    const found = new Map<string, EventObject>();
    try {
      await Promise.all(
        statusesToRead(counts).map(async (status) => {
          let after: string | undefined;
          do {
            const page = checkObjectPage(
              await call(
                "listObjects",
                {
                  path: { event },
                  query:
                    after === undefined ? { status, limit: 100 } : { status, limit: 100, after },
                },
                signal,
              ),
            );
            requests += 1;
            for (const o of page.data) found.set(o.label, o);
            after = page.next ?? undefined;
          } while (after !== undefined);
        }),
      );
      const previous = nonFree;
      nonFree = mergeRead(found, previous, learnt, started);
      for (const [label, at] of learnt) if (at < started) learnt.delete(label);
      sync(new Set([...drawn, ...nonFree.keys()]));
      countsAtRead = counts;
      const endedAt = Date.now();
      last = { requests, objects: found.size, tookMs: endedAt - started, endedAt };
      history.push(last);
      wholeFailed = null;
      if (history.length > 20) history.shift();
      bump();
    } finally {
      reading = false;
    }
  }

  const failed = (e: unknown): number | null => {
    if (isAbort(e) || signal.aborted) return null;
    const error = asApiError(e);
    setHealth((h) => ({ ...h, error }));
    return error.retryAfter === null ? null : error.retryAfter * 1000;
  };

  async function tick(): Promise<void> {
    timer = null;
    if (stopped || paused) return;
    let delay = TICK_MS;
    try {
      const answer = checkEvent(await call("getEvent", { path: { event } }, signal));
      options.onEvent(answer);
      const watched = [...selected];
      if (inspected !== null) watched.push(inspected);
      if (watched.length > 0) await read(watched);
      const counts = answer.counts;
      if (
        counts !== undefined &&
        !reading &&
        wholeReadDue(Date.now(), last, countsAtRead === null || !sameCounts(counts, countsAtRead))
      ) {
        void whole(counts).catch((e: unknown) => {
          failed(e);
          if (!isAbort(e) && !signal.aborted) wholeFailed = asApiError(e);
        });
      }
      setHealth({ checkedAt: Date.now(), error: wholeFailed });
    } catch (e) {
      delay = Math.max(delay, failed(e) ?? 0);
    }
    schedule(delay);
  }

  function schedule(ms: number): void {
    if (stopped || paused || signal.aborted) return;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => void tick(), ms);
  }

  function stop(): void {
    stopped = true;
    if (timer !== null) clearTimeout(timer);
    timer = null;
  }
  signal.addEventListener("abort", stop, { once: true });

  const adapter: StateAdapter = {
    async connect(context) {
      const answer = checkEvent(await call("getEvent", { path: { event } }, signal));
      options.onEvent(answer);
      // The first read fills `drawn` without telling the map: the snapshot
      // below is what it starts from.
      if (answer.counts !== undefined) await whole(answer.counts);
      ctx = context;
      setHealth({ checkedAt: Date.now(), error: null });
      schedule(TICK_MS);
      return { kind: "map", unavailable: [...drawn] };
    },
    pause() {
      paused = true;
      if (timer !== null) clearTimeout(timer);
      timer = null;
    },
    resume() {
      paused = false;
      schedule(0);
    },
    disconnect: stop,
  };

  const PATCH: Record<
    Action,
    (label: string, oid: number, orderRef?: string) => EventObject | null
  > = {
    block: (label, oid) => ({ label, oid, status: "blocked" }),
    unblock: () => null,
    release: () => null,
    book: (label, oid, orderRef) =>
      orderRef === undefined
        ? { label, oid, status: "booked" }
        : { label, oid, status: "booked", order_ref: orderRef },
  };

  return {
    adapter,
    statusOf: (label) => nonFree.get(label)?.status ?? "free",
    object: (label) => nonFree.get(label),
    version,
    health,
    setSelected(labels) {
      const next = new Set(labels);
      const changed = new Set([...selected, ...next]);
      selected = next;
      sync(changed);
    },
    allow(labels) {
      // Drawn available ahead of the selection that will keep them so.
      const delta = { unavailable: [] as string[], available: labels.filter((l) => drawn.has(l)) };
      for (const l of delta.available) drawn.delete(l);
      if (ctx !== null && delta.available.length > 0) ctx.applyDelta(delta);
    },
    settle: (labels) => sync(labels),
    inspect(label) {
      inspected = label;
    },
    read: (labels) => read(labels),
    async act(action, labels, orderRef) {
      const objects = [...labels];
      const headers = { "Idempotency-Key": crypto.randomUUID() };
      const path = { event };
      try {
        const answer = checkSeatReply(
          action === "block" || action === "unblock"
            ? await call(
                "changeStatus",
                {
                  path,
                  headers,
                  body: { objects, status: action === "block" ? "blocked" : "free" },
                },
                signal,
              )
            : action === "release"
              ? await call("release", { path, headers, body: { objects } }, signal)
              : await call(
                  "book",
                  { path, headers, body: { objects, order_ref: orderRef ?? "" } },
                  signal,
                ),
        );
        const at = Date.now();
        const patched = answer.objects
          .map((o) => PATCH[action](o.label, o.oid, orderRef))
          .filter((o): o is EventObject => o !== null);
        learn(patched, objects, at);
        sync(objects);
        bump();
        // The details the reply does not carry (`seq`), and a check.
        void read(objects).catch(failed);
        schedule(0);
        return { ok: true, labels: answer.objects.map((o) => o.label) };
      } catch (e) {
        if (isAbort(e)) throw e;
        const error = asApiError(e);
        // Whatever the refusal, show what the seats are now.
        await read(objects).catch(failed);
        return { ok: false, error, refused: refusedSeats(error) };
      }
    },
    reads: () => history,
  };
}
