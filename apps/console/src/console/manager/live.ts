// The event manager's view of the seats (09 §4.2, CON-008; D-68, D-80),
// kept current by polling the staff state route (`GET …/state`, 08 §8.4,
// D-72): one manager SNAPSHOT at the start, then every `POLL_MS` the DELTA
// since the `seq` held, `204` when nothing changed, or a SNAPSHOT when the
// server no longer keeps that far back (or restarted). The frames name
// objects by `idx`; the chart's labels by `idx` come from the same bytes the
// map draws (`chartLabels`). Order references, ticket types and hold
// expiries are not in the frames: they are read by label for the seats the
// operator is looking at (the selection and the seat inspected), when those
// change and when the state changes one of them.
//
// It feeds the renderer as a `customState` adapter by label: the map draws
// a seat unavailable when it is not free and not selected, so the picker
// lets the operator select any seat (D-68). Nothing runs while the page is
// hidden; everything stops with `signal`.
import type { AdapterContext, StateAdapter } from "@cavea/renderer";
import { createSignal } from "solid-js";
import { checkObjectsByLabel, checkSeatReply } from "../../api/check.ts";
import { ApiError, call, callBytes, isAbort } from "../../api/client.ts";
import type { Counts, EventObject } from "../../api/generated.ts";
import { asApiError } from "../../api/query.ts";
import { chartLabels } from "./labels.ts";
import {
  countsOf,
  deltaFor,
  refusedSeats,
  seatStatus,
  statusOf,
  type Action,
  type Pending,
  type Refusal,
  type Status,
} from "./model.ts";
import { nextAfter, POLL_MS, step, type Step } from "./sync.ts";

/** One poll, for the measurements (dev builds expose them). */
export interface Poll {
  /** `after_seq` sent. */
  after: number;
  kind: Step["kind"] | "error";
  bytes: number;
  tookMs: number;
  at: number;
}

export interface Health {
  checkedAt: number | null;
  error: ApiError | null;
}

export type Outcome =
  | { ok: true; labels: string[] }
  | { ok: false; error: ApiError; refused: Refusal[] };

export interface Live {
  /** The map's state adapter; its `chart` gives the map the chart's bytes. */
  adapter: StateAdapter;
  /** A seat's status as the state (or the operator's own action) says. */
  statusOf(label: string): Status;
  /** What was read of a seat that is not free (order, ticket type, expiry),
   * while it is still in the status it was read in. */
  object(label: string): EventObject | undefined;
  /** Changes whenever a status or detail known to the page changes. */
  version(): number;
  /** The counts as the state shows them; `null` before the first state. */
  counts(): Counts | null;
  /** When the state was last checked, and the last failure if the latest
   * check failed (what is shown may then be stale). */
  health(): Health;
  /** The map's selection: these seats are drawn available whatever their
   * status, so that the picker keeps them; the others go back. */
  setSelected(labels: readonly string[]): void;
  /** Draws `labels` available now, so that the picker takes them when told
   * to select them; `settle` then puts back any it did not take. */
  allow(labels: readonly string[]): void;
  settle(labels: readonly string[]): void;
  inspect(label: string | null): void;
  /** Reads the details of `labels` now (a seat just clicked). */
  read(labels: readonly string[]): Promise<void>;
  /** Acts on `labels`, all or nothing, and reads them again either way. */
  act(action: Action, labels: readonly string[], orderRef?: string): Promise<Outcome>;
  /** The last 50 polls, for measurement. */
  polls(): readonly Poll[];
  /** The label of object `idx`, for measurement. */
  labelAt(idx: number): string | undefined;
}

/** Labels per read by label: the server takes at most 1,000. */
const LABELS_PER_READ = 1_000;

const badFrame = (rule: string, requestId: string | null) =>
  new ApiError(
    200,
    "unexpected",
    rule,
    {},
    requestId,
    null,
    "The server's answer could not be read.",
  );

export function createLive(options: {
  event: string;
  /** The event's chart version (`version.url`). */
  chartUrl: string;
  signal: AbortSignal;
  /** The counts and `seq` after each change of the state. */
  onCounts: (counts: Counts, seq: number) => void;
}): Live {
  const { event, signal } = options;
  let labels: string[] = [];
  let index = new Map<string, number>();
  let codes: Uint8Array | null = null;
  let seq: number | null = null;
  let lastKind: Step["kind"] | null = null;
  const pending = new Map<string, Pending>();
  const details = new Map<string, EventObject>();
  const drawn = new Set<string>();
  let selected = new Set<string>();
  let inspected: string | null = null;
  let ctx: AdapterContext | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** This connection's requests (a map that failed to start disconnects;
   * Retry connects again). */
  let conn = new AbortController();
  let chart: Promise<Uint8Array> | null = null;
  let paused = false;
  let stopped = false;
  const history: Poll[] = [];
  const [version, setVersion] = createSignal(0);
  const [counts, setCounts] = createSignal<Counts | null>(null);
  const [health, setHealth] = createSignal<Health>({ checkedAt: null, error: null });
  const bump = () => setVersion((v) => v + 1);

  const codeOf = (label: string): number => {
    const i = index.get(label);
    return i === undefined || codes === null ? 0 : (codes[i] ?? 0);
  };
  const status = (label: string): Status =>
    seatStatus(pending, seq, label, () => statusOf(codeOf(label)));
  const hidden = (label: string) => status(label) !== "free" && !selected.has(label);
  /** Tells the map about `labels`. */
  const sync = (changed: Iterable<string>) => {
    const delta = deltaFor(changed, drawn, hidden);
    if (ctx !== null && (delta.unavailable.length > 0 || delta.available.length > 0)) {
      ctx.applyDelta(delta);
    }
  };
  const watched = (): string[] => {
    const out = [...selected];
    if (inspected !== null && !selected.has(inspected)) out.push(inspected);
    return out;
  };

  async function read(wanted: readonly string[]): Promise<void> {
    const unique = [...new Set(wanted)].filter((l) => index.has(l));
    for (let i = 0; i < unique.length; i += LABELS_PER_READ) {
      const chunk = unique.slice(i, i + LABELS_PER_READ);
      const answer = checkObjectsByLabel(
        await call("listObjects", { path: { event }, query: { label: chunk } }, conn.signal),
      );
      for (const l of chunk) details.delete(l);
      for (const o of answer.data) details.set(o.label, o);
    }
    bump();
  }
  const readQuietly = (wanted: readonly string[]) => {
    if (wanted.length > 0) void read(wanted).catch(failed);
  };

  /** The chart, fetched once for the map and the labels; `abort` is the
   * map's signal (the adapter's `chart`) or the connection's. */
  function loadChart(abort: AbortSignal): Promise<Uint8Array> {
    // `/v1/public/charts/{file}`: the version's own URL names the file.
    const file = decodeURIComponent(options.chartUrl.split("/").pop() ?? "");
    chart ??= callBytes("publicChart", { path: { file } }, abort).then(({ bytes }) => {
      if (bytes === null) throw badFrame("chart", null);
      labels = chartLabels(bytes);
      index = new Map(labels.map((l, i) => [l, i]));
      return bytes;
    });
    // A failure is not kept: Retry fetches again.
    chart.catch(() => (chart = null));
    return chart;
  }

  /** Applies a step to the state; returns the labels whose status changed. */
  function apply(s: Step): string[] {
    lastKind = s.kind;
    if (s.kind === "snapshot") {
      const before = codes;
      codes = s.codes;
      seq = s.seq;
      if (before === null) return [];
      const changed: string[] = [];
      for (let i = 0; i < s.codes.length; i++) {
        if (s.codes[i] !== before[i]) changed.push(labels[i] ?? "");
      }
      return changed;
    }
    if (s.kind === "delta" && codes !== null) {
      const changed: string[] = [];
      for (let k = 0; k < s.idx.length; k++) {
        const i = s.idx[k] ?? 0;
        const c = s.codes[k] ?? 0;
        if (codes[i] !== c) changed.push(labels[i] ?? "");
        codes[i] = c;
      }
      seq = s.to;
      return changed;
    }
    return [];
  }

  /** One poll; `ready` (the chart's labels) is awaited after the request
   * is sent, so the first snapshot is fetched beside the chart. */
  async function poll(ready: Promise<unknown> = Promise.resolve()): Promise<Step> {
    const after = nextAfter(seq, lastKind);
    const started = performance.now();
    const [answer] = await Promise.all([
      callBytes("eventState", { path: { event }, query: { after_seq: after } }, conn.signal),
      ready,
    ]);
    const s =
      answer.bytes === null
        ? step(seq, labels.length, { status: 204 })
        : step(seq, labels.length, { status: 200, bytes: answer.bytes });
    history.push({
      after,
      kind: s.kind,
      bytes: answer.bytes?.length ?? 0,
      tookMs: performance.now() - started,
      at: Date.now(),
    });
    if (history.length > 50) history.shift();
    if (s.kind === "bad") {
      // Nothing of it is applied; the next poll asks for a snapshot.
      lastKind = "bad";
      throw badFrame(s.rule, answer.requestId);
    }
    if (s.kind === "gap") lastKind = "gap";
    return s;
  }

  /** After the state changed: the map, the counts, the watched details. */
  function stateChanged(labelsChanged: string[], whole: boolean): void {
    if (codes === null || seq === null) return;
    sync(whole ? labels : labelsChanged);
    options.onCounts(countsOf(codes), seq);
    setCounts(countsOf(codes));
    const look = watched();
    const touched = new Set(labelsChanged);
    // A watched seat changed, or one is held and its hold may have been
    // extended (a `seq` that moved without a status change).
    if (
      whole ||
      look.some((l) => touched.has(l)) ||
      (labelsChanged.length === 0 && look.some((l) => status(l) === "held"))
    ) {
      readQuietly(look);
    }
    bump();
  }

  const failed = (e: unknown): number | null => {
    if (isAbort(e) || conn.signal.aborted) return null;
    const error = asApiError(e);
    setHealth((h) => ({ ...h, error }));
    return error.retryAfter === null ? null : error.retryAfter * 1000;
  };

  /** One poll at a time: a poll asked for while one runs follows it. */
  let polling = false;
  let again = false;
  async function tick(): Promise<void> {
    timer = null;
    if (stopped || paused) return;
    if (polling) {
      again = true;
      return;
    }
    polling = true;
    let delay = POLL_MS;
    try {
      const before = seq;
      const s = await poll();
      if (stopped || paused) return;
      if (s.kind === "gap") {
        delay = 0;
      } else {
        // Seats the operator's own action is shown for: once the state has
        // caught up, it decides, even when its delta left their code as it
        // was (the action and a later change merged into one).
        const waiting = [...pending.keys()];
        const moved = apply(s);
        if (s.kind === "snapshot" || (s.kind === "delta" && seq !== before)) {
          stateChanged([...moved, ...waiting], s.kind === "snapshot");
        }
      }
      setHealth({ checkedAt: Date.now(), error: null });
    } catch (e) {
      delay = Math.max(delay, failed(e) ?? 0);
    } finally {
      polling = false;
    }
    if (again) {
      again = false;
      delay = Math.min(delay, 0);
    }
    schedule(delay);
  }

  function schedule(ms: number): void {
    if (stopped || paused || conn.signal.aborted) return;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => void tick(), ms);
  }

  function stop(): void {
    stopped = true;
    if (timer !== null) clearTimeout(timer);
    timer = null;
    conn.abort();
  }
  signal.addEventListener("abort", stop, { once: true });

  const adapter: StateAdapter = {
    chart: async (abort) => ({ bytes: await loadChart(abort) }),
    async connect(context) {
      // A fresh start: after a map that failed to start, Retry connects again.
      conn.abort();
      conn = new AbortController();
      if (signal.aborted) conn.abort();
      stopped = signal.aborted;
      paused = false;
      ctx = null;
      codes = null;
      seq = null;
      lastKind = null;
      pending.clear();
      details.clear();
      drawn.clear();
      selected = new Set();
      const s = await poll(loadChart(conn.signal));
      if (s.kind !== "snapshot") throw badFrame("first state", null);
      apply(s);
      // The first state fills `drawn` without telling the map: the snapshot
      // below is what it starts from.
      stateChanged([], true);
      ctx = context;
      setHealth({ checkedAt: Date.now(), error: null });
      schedule(POLL_MS);
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

  const AFTER: Record<Action, Status> = {
    block: "blocked",
    unblock: "free",
    release: "free",
    book: "booked",
  };

  return {
    adapter,
    statusOf: status,
    object(label) {
      const o = details.get(label);
      return o !== undefined && o.status === status(label) ? o : undefined;
    },
    version,
    counts,
    health,
    setSelected(next) {
      const now = new Set(next);
      const added = next.filter((l) => !selected.has(l));
      const all = new Set([...selected, ...now]);
      selected = now;
      sync(all);
      readQuietly(added);
    },
    allow(wanted) {
      // Drawn available ahead of the selection that will keep them so.
      const delta = { unavailable: [] as string[], available: wanted.filter((l) => drawn.has(l)) };
      for (const l of delta.available) drawn.delete(l);
      if (ctx !== null && delta.available.length > 0) ctx.applyDelta(delta);
    },
    settle: (wanted) => sync(wanted),
    inspect(label) {
      inspected = label;
    },
    read: (wanted) => read(wanted),
    async act(action, targets, orderRef) {
      const objects = [...targets];
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
                conn.signal,
              )
            : action === "release"
              ? await call("release", { path, headers, body: { objects } }, conn.signal)
              : await call(
                  "book",
                  { path, headers, body: { objects, order_ref: orderRef ?? "" } },
                  conn.signal,
                ),
        );
        for (const o of answer.objects) {
          pending.set(o.label, { status: AFTER[action], seq: answer.seq });
        }
        sync(objects);
        bump();
        // The details the reply does not carry (order, ticket type).
        readQuietly(objects);
        schedule(0);
        return { ok: true, labels: answer.objects.map((o) => o.label) };
      } catch (e) {
        if (isAbort(e)) throw e;
        const error = asApiError(e);
        // Whatever the refusal, show what the seats are now: the state at
        // once, and their details.
        schedule(0);
        await read(objects).catch(failed);
        return { ok: false, error, refused: refusedSeats(error) };
      }
    },
    polls: () => history,
    labelAt: (idx) => labels[idx],
  };
}
