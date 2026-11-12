// `caveaServer` (docs/05-web-sdk.md §4): seat state from `cavea-server` over
// WebSocket, SSE or polling (08 §8), holds over HTTPS (08 §6.2), the hold
// token kept across reloads. Built only on the public adapter interface.
import type { AdapterContext, HoldResult, StateAdapter } from "./adapter.ts";
import { CaveaError } from "./errors.ts";
import { fromServer, reportLicence } from "./licence.ts";
import {
  CLOSE,
  DELTA,
  HELLO,
  PING,
  PRIVATE,
  SNAPSHOT,
  fromBase64,
  pong,
  readHello,
  readSeq,
  subscribe,
} from "./wire.ts";

export interface CaveaServerOptions {
  /** Where `cavea-server` answers, such as `https://tickets.example.com/cavea`. */
  baseUrl: string;
  /** A publishable key of the event's workspace (`pk_…`). */
  publishableKey: string;
  /** The event key. */
  event: string;
  /**
   * `start` (default): a hold token is created at the buyer's first tap and
   * kept in `sessionStorage`; `continue`: `holdToken`, from the host's backend;
   * `manual`: nothing is held, the host holds at checkout; `none`: state only.
   */
  session?: "start" | "continue" | "manual" | "none";
  /** The hold token to continue with: required with `session: "continue"`, refused otherwise. */
  holdToken?: string;
  /** Sent as `Cavea-Session`; a function is called at the first hold, and once more when the token expired. */
  sessionToken?: string | (() => Promise<string>);
  /** Default `auto`: WebSocket, else SSE, else polling. */
  transport?: "auto" | "ws" | "sse" | "poll";
  /** Default true: drop the realtime connection while the page is hidden. */
  pauseWhenHidden?: boolean;
}

/** What the adapter takes from the platform: the browser's, or fakes in tests. */
export interface Platform {
  fetch(url: string, init: RequestInit): Promise<Response>;
  WebSocket: typeof WebSocket | undefined;
  EventSource: typeof EventSource | undefined;
  /** May throw (sandboxed frames). */
  storage(): Storage | undefined;
  setTimeout(run: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
  now(): number;
  random(): number;
}

const browser = (): Platform => ({
  fetch: (url, init) => fetch(url, init),
  WebSocket: globalThis.WebSocket,
  EventSource: globalThis.EventSource,
  storage: () => globalThis.sessionStorage,
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
  now: () => Date.now(),
  random: () => Math.random(),
});

/** Polling interval (08 §8.4). */
const POLL_MS = 2_000;
/** Reconnection delay: doubling from the first to the second, times 0.5 to 1 at random. */
const BACKOFF_MS = [500, 30_000] as const;
/** Consecutive failures, with no HELLO ever on a transport, after which `auto` gives it up. */
const FALLBACK_AFTER = 2;
/** A WebSocket silent this long is dead: the server pings every 25 s. */
const SILENCE_MS = 60_000;
/** A hidden page keeps its connection this long. */
const PAUSE_MS = 10_000;
/** One request's limit, its body included. */
const REQUEST_MS = 8_000;
/** A hold or release, with token creation and retries, gives up after this: under the map's 20 s. */
const HOLD_MS = 18_000;
/** A hold, release or token creation that failed for want of capacity is retried once, after at most this. */
const RETRY_MS = 2_000;
/** Without PRIVATE frames, the buyer's holds are read again this often while any is live. */
const MINE_MS = 60_000;
/** A failed read of the buyer's holds is tried again after this. */
const MINE_RETRY_MS = 5_000;

/** Hold tokens when `sessionStorage` cannot be used. */
const memory = new Map<string, string>();

interface EventInfo {
  chart: { hash: string; url: string };
  hold_ttl_seconds: number;
  require_session: boolean;
  /** `{status, domains}` from a server that licenses (12 §7.6); untrusted, read by `fromServer`. */
  licence?: unknown;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

function time(value: unknown): number {
  const t = Date.parse(String(value));
  if (!Number.isFinite(t)) {
    throw new Error(`not a time: ${String(value)}`);
  }
  return t;
}

/** A problem+json answer (08 §6.4), read safely. */
async function problem(
  res: Response,
): Promise<{ status: number; code: string; details: Record<string, unknown> }> {
  const body: unknown = await res.json().catch(() => undefined);
  const code = isObject(body) && typeof body["code"] === "string" ? body["code"] : "";
  const details = isObject(body) && isObject(body["details"]) ? body["details"] : {};
  return { status: res.status, code: code || `http_${res.status}`, details };
}

const said = (res: Response, p: { code: string }): string => `${p.code} (HTTP ${res.status})`;

const fail = (name: string): CaveaError =>
  new CaveaError(
    "invalid_option",
    `option "state.${name}" is invalid`,
    true,
    undefined,
    `state.${name}`,
  );

/** The options, checked, and the base URL without a trailing slash. */
function validate(input: unknown): [CaveaServerOptions, string] {
  if (!isObject(input)) {
    throw fail("options");
  }
  const { session = "start", sessionToken, transport, pauseWhenHidden } = input;
  for (const name of ["baseUrl", "publishableKey", "event"]) {
    if (typeof input[name] !== "string" || input[name] === "") {
      throw fail(name);
    }
  }
  let root: string;
  try {
    root = new URL(String(input["baseUrl"]), globalThis.location?.href).href.replace(/\/+$/, "");
  } catch {
    throw fail("baseUrl");
  }
  if (!["start", "continue", "manual", "none"].includes(session as string)) {
    throw fail("session");
  }
  if (
    session === "continue"
      ? typeof input["holdToken"] !== "string"
      : input["holdToken"] !== undefined
  ) {
    throw fail("holdToken");
  }
  if (!["undefined", "string", "function"].includes(typeof sessionToken)) {
    throw fail("sessionToken");
  }
  if (transport !== undefined && !["auto", "ws", "sse", "poll"].includes(transport as string)) {
    throw fail("transport");
  }
  if (!["undefined", "boolean"].includes(typeof pauseWhenHidden)) {
    throw fail("pauseWhenHidden");
  }
  return [input as unknown as CaveaServerOptions, root];
}

/** Seat state and holds from `cavea-server` (05 §4). Invalid options throw a fatal `CaveaError`. */
export function caveaServer(options: CaveaServerOptions): StateAdapter {
  return serverAdapter(options, browser);
}

export function serverAdapter(input: CaveaServerOptions, platform: () => Platform): StateAdapter {
  const [options, root] = validate(input);
  const { event, publishableKey: key } = options;
  const mode = options.session ?? "start";
  const eventUrl = `${root}/v1/public/events/${encodeURIComponent(event)}`;
  const currentUrl = `${root}/v1/public/hold-tokens/current`;
  const keyParam = `key=${encodeURIComponent(key)}`;
  const storageKey = `cavea:hold:${root}|${key}|${event}`;

  let P!: Platform;
  let ctx!: AdapterContext;
  let signal: AbortSignal | undefined;
  let info: Promise<EventInfo> | undefined;
  let token: string | null = null;
  let session = typeof options.sessionToken === "string" ? options.sessionToken : null;
  /** The `seq` of the last state forwarded in order; 0 asks for a SNAPSHOT. */
  let seq = 0;
  let order: Array<"ws" | "sse" | "poll"> = [];
  let transport: "ws" | "sse" | "poll" = "poll";
  /** Transports that delivered a HELLO (or a poll answer) on this page. */
  const worked = new Set<string>();
  let failures = 0;
  /** The realtime connection, or the poll loop. */
  let link: { close(): void } | undefined;
  /** `link` delivered its HELLO (or a poll answer). */
  let live = false;
  /** `link` carries PRIVATE frames: a WebSocket that subscribes with the hold token. */
  let privateLive = false;
  let offset: number | undefined;
  let chartHash: string | undefined;
  /** The event's `licence` field, as the server sent it. */
  let licence: unknown;
  /** The buyer's held count and expiry (server ms), from `hold-tokens/current`. */
  let held = 0;
  let expires = 0;
  /** A hold or release is running; `epoch` counts them, so an older read of the holds is dropped. */
  let busy = false;
  let epoch = 0;
  /** When the running hold or release gives up (`P.now()` ms). */
  let deadline = Infinity;
  let stopped = false;
  let halted = false;
  let paused = false;
  const timers = new Map<string, unknown>();

  function later(name: string, ms: number, run: () => void): void {
    cancel(name);
    timers.set(
      name,
      P.setTimeout(() => {
        timers.delete(name);
        run();
      }, ms),
    );
  }
  function cancel(name: string): void {
    if (timers.has(name)) {
      P.clearTimeout(timers.get(name));
      timers.delete(name);
    }
  }
  const sleep = (ms: number): Promise<void> => new Promise((done) => P.setTimeout(done, ms));
  const report = (
    code: "state_failed" | "state_disconnected" | "hold_failed",
    message: string,
  ): void => {
    if (!stopped) {
      ctx.reportError(new CaveaError(code, message));
    }
  };
  /** Runs `work` in the background; a failure is reported. */
  const run = (work: Promise<unknown>): void => {
    work.catch((error: unknown) => {
      if (!stopped) {
        ctx.reportError(error);
      }
    });
  };

  // ---- storage ----

  /** `sessionStorage`, or memory where it cannot be used (private modes, sandboxed frames). */
  function storage(): Pick<Storage, "getItem" | "setItem" | "removeItem"> {
    try {
      const store = P.storage();
      if (store !== undefined) {
        // Throws where storage is denied or full.
        store.setItem(`${storageKey}?`, "");
        store.removeItem(`${storageKey}?`);
        return store;
      }
    } catch {
      // Memory below.
    }
    return {
      getItem: (k) => memory.get(k) ?? null,
      setItem: (k, v) => memory.set(k, v),
      removeItem: (k) => memory.delete(k),
    };
  }
  function read(): string | null {
    return storage().getItem(storageKey);
  }
  function write(value: string | null): void {
    const store = storage();
    if (value === null) {
      store.removeItem(storageKey);
    } else {
      store.setItem(storageKey, value);
    }
  }

  // ---- HTTP ----

  /**
   * A request with the key in `Authorization` (`auth`) and its body read in
   * full, cancelled on destroy, after `REQUEST_MS` or at the hold's deadline.
   */
  async function request(url: string, init: RequestInit = {}, auth = true): Promise<Response> {
    const ms = Math.min(REQUEST_MS, deadline - P.now());
    if (stopped || signal?.aborted || ms <= 0) {
      throw new Error(stopped || signal?.aborted ? "the map was destroyed" : "out of time");
    }
    const abort = new AbortController();
    const stop = (): void => abort.abort();
    signal?.addEventListener("abort", stop);
    const timer = P.setTimeout(stop, ms);
    try {
      const headers = new Headers(init.headers);
      if (auth) {
        headers.set("Authorization", `Bearer ${key}`);
      }
      const res = await P.fetch(url, { ...init, headers, signal: abort.signal });
      const body = await res.arrayBuffer();
      return new Response(res.status === 204 || res.status === 304 ? null : body, {
        status: res.status,
        statusText: res.statusText,
        headers: res.headers,
      });
    } finally {
      P.clearTimeout(timer);
      signal?.removeEventListener("abort", stop);
    }
  }

  async function sessionToken(): Promise<string | null> {
    if (session === null && typeof options.sessionToken === "function") {
      // The host's function shares the hold's deadline.
      const left = deadline - P.now();
      let timer: unknown;
      const late = new Promise<never>((_, reject) => {
        if (Number.isFinite(left)) {
          timer = P.setTimeout(() => reject(new Error("sessionToken() did not answer")), left);
        }
      });
      try {
        session = String(await Promise.race([options.sessionToken(), late]));
      } finally {
        P.clearTimeout(timer);
      }
    }
    return session;
  }

  /**
   * A hold-path POST: retried once after a network failure, `429` or `503`
   * when the wait (`Retry-After`) is at most `RETRY_MS`; once more with a new
   * session token after `401` `expired`.
   */
  async function post(url: string, body: unknown): Promise<Response> {
    let retried = false;
    let refreshed = false;
    for (;;) {
      const headers: Record<string, string> = { "Content-Type": "application/json" };
      const s = await sessionToken();
      if (s !== null) {
        headers["Cavea-Session"] = s;
      }
      let wait = 500;
      try {
        const res = await request(url, { method: "POST", headers, body: JSON.stringify(body) });
        if (res.status === 401 && !refreshed && typeof options.sessionToken === "function") {
          if ((await problem(res.clone())).details["session"] === "expired") {
            refreshed = true;
            session = null;
            continue;
          }
        }
        if (res.status !== 429 && res.status !== 503) {
          return res;
        }
        wait = Number(res.headers.get("Retry-After") ?? 1) * 1000 || 1000;
        if (retried || wait > RETRY_MS || P.now() + wait >= deadline) {
          return res;
        }
      } catch (error) {
        if (retried || stopped || P.now() + wait >= deadline) {
          throw error;
        }
      }
      retried = true;
      await sleep(wait);
    }
  }

  function eventInfo(): Promise<EventInfo> {
    if (info !== undefined) {
      return info;
    }
    info = request(eventUrl).then(async (res) => {
      if (!res.ok) {
        const error = new Error(`event "${event}": ${said(res, await problem(res))}`);
        throw Object.assign(error, { status: res.status });
      }
      const body = (await res.json()) as EventInfo;
      if (typeof body.chart?.url !== "string") {
        throw new Error(`event "${event}": no chart`);
      }
      return body;
    });
    // A failure is not kept: the map tries again (05 §9).
    info.catch(() => {
      info = undefined;
    });
    return info;
  }

  function clock(ms: number): void {
    offset = ms;
    ctx.setClockOffset(ms);
  }

  // ---- the hold token ----

  function setToken(value: string | null): void {
    token = value;
    ctx.setHoldToken(value);
    if (mode === "start") {
      write(value);
    }
  }

  /** The token expired or is unknown: the session has ended and its holds are gone. */
  function ended(): void {
    if (token === null) {
      return;
    }
    setToken(null);
    held = 0;
    cancel("mine");
    ctx.setMine([]);
    if (mode === "continue") {
      report("state_failed", "the hold token has ended: holding needs a new one");
    }
  }

  /** `hold-tokens/current`'s answer: the buyer's holds, into the engine. */
  function ownHolds(body: unknown): void {
    if (!isObject(body) || body["event"] !== event || !Array.isArray(body["objects"])) {
      throw new Error("hold-tokens/current: unexpected answer");
    }
    const labels = body["objects"].map((o: unknown) => String(isObject(o) ? o["label"] : o));
    held = labels.length;
    expires = time(body["expires_at"]);
    ctx.setMine(labels, expires);
    watchMine();
  }

  /** Reads the buyer's holds where no PRIVATE frame brings them (SSE, polling, a WebSocket without the token). */
  async function refreshMine(): Promise<void> {
    const t = token;
    const at = epoch;
    if (t === null || privateLive || busy || stopped) {
      return;
    }
    const res = await request(currentUrl, { headers: { "Cavea-Hold-Token": t } }).catch(
      () => undefined,
    );
    const body: unknown = res?.ok ? await res.json().catch(() => undefined) : undefined;
    // A hold or release since the read began makes the answer stale; its own read follows.
    if (stopped || t !== token || at !== epoch || busy || privateLive) {
      return;
    }
    if (res?.status === 404) {
      ended();
    } else if (res?.ok) {
      ownHolds(body);
    } else {
      // Failed: read again soon, whether or not any hold is known yet.
      later("mine", MINE_RETRY_MS, () => run(refreshMine()));
    }
  }

  /** Reads the holds again at their expiry, and every `MINE_MS` while any is live. */
  function watchMine(): void {
    cancel("mine");
    if (!stopped && token !== null && !privateLive && held > 0) {
      const toExpiry = expires - (P.now() + (offset ?? 0)) + 1000;
      later("mine", Math.max(1000, Math.min(MINE_MS, toExpiry)), () => run(refreshMine()));
    }
  }

  /** A stored or given token: kept if the server knows it, with its holds. */
  async function restore(stored: string): Promise<void> {
    const res = await request(currentUrl, { headers: { "Cavea-Hold-Token": stored } }).catch(
      () => undefined,
    );
    if (res?.status === 404) {
      if (mode === "start") {
        write(null);
      } else {
        report("state_failed", "the hold token given (holdToken) is not live");
      }
      return;
    }
    setToken(stored);
    // A WebSocket subscribes with the token and gets its holds in a PRIVATE frame:
    // setting them here too would change the expiry twice (this answer is to the second).
    if (res?.ok && transport !== "ws") {
      ownHolds(await res.json());
    }
  }

  /** A refusal as the map takes it: per-object reasons by label; `not_found` only for a token gone. */
  async function refusal(res: Response): Promise<HoldResult> {
    const p = await problem(res);
    if (p.status === 404 && p.details["missing"] === "hold_token") {
      ended();
      return { ok: false, code: "not_found" };
    }
    const code = p.code === "not_found" ? "unknown_object" : p.code;
    if (p.status !== 409 && p.status !== 422) {
      report("hold_failed", `hold refused: ${said(res, p)}`);
    }
    const objects = Array.isArray(p.details["objects"]) ? p.details["objects"] : [];
    const failed = objects
      .filter(isObject)
      .map((o) => ({ label: String(o["label"]), reason: String(o["reason"] ?? code) }));
    return failed.length > 0 ? { ok: false, code, failed } : { ok: false, code };
  }

  /** The hold token, created at the first hold in `start` mode; or the refusal. */
  async function ensureToken(): Promise<string | HoldResult> {
    if (token !== null) {
      return token;
    }
    if (mode === "continue") {
      return { ok: false, code: "session_ended" };
    }
    const sent = P.now();
    const res = await post(`${root}/v1/public/hold-tokens`, { event });
    if (!res.ok) {
      return refusal(res);
    }
    const body = (await res.json()) as { hold_token?: unknown; expires_at?: unknown };
    if (typeof body.hold_token !== "string") {
      throw new Error("hold-tokens: no hold_token in the answer");
    }
    // Until a HELLO says better: the expiry (to the second) less the lifetime is the server's time of creation.
    if (offset === undefined) {
      const ttl = (await eventInfo()).hold_ttl_seconds * 1000;
      clock(time(body.expires_at) + 500 - ttl - (sent + P.now()) / 2);
    }
    setToken(body.hold_token);
    // Subscribe again, with the token, for PRIVATE frames.
    if (transport === "ws" && link !== undefined) {
      reopen();
    }
    return body.hold_token;
  }

  async function hold(labels: string[]): Promise<HoldResult> {
    busy = true;
    epoch += 1;
    deadline = P.now() + HOLD_MS;
    try {
      for (let fresh = mode === "start"; ; fresh = false) {
        const t = await ensureToken();
        if (typeof t !== "string") {
          return t;
        }
        const res = await post(`${eventUrl}/hold`, { hold_token: t, objects: labels });
        if (res.ok) {
          held += labels.length;
          return { ok: true };
        }
        const result = await refusal(res);
        // In `start` mode a token that has ended (expired, or booked) is replaced
        // at once, so the tap holds; the old holds have already left (`ended`).
        if (!(fresh && !result.ok && result.code === "not_found")) {
          return result;
        }
      }
    } finally {
      busy = false;
      deadline = Infinity;
      run(refreshMine());
    }
  }

  async function release(labels: string[]): Promise<void> {
    busy = true;
    epoch += 1;
    deadline = P.now() + HOLD_MS;
    try {
      if (token === null) {
        return;
      }
      const res = await post(`${eventUrl}/release`, { hold_token: token, objects: labels });
      if (res.ok) {
        return;
      }
      const p = await problem(res);
      if (p.status === 404 && p.details["missing"] === "hold_token") {
        return ended();
      }
      throw new Error(`release refused: ${said(res, p)}`);
    } finally {
      busy = false;
      deadline = Infinity;
      run(refreshMine());
    }
  }

  // ---- realtime ----

  function open(): void {
    if (stopped || halted || paused || link !== undefined) {
      return;
    }
    cancel("retry");
    try {
      if (transport === "ws") {
        openWs();
      } else if (transport === "sse") {
        openSse();
      } else {
        openPoll();
      }
    } catch (error) {
      report("state_failed", `realtime connection: ${String(error)}`);
      dropped();
    }
  }

  function close(): void {
    const l = link;
    link = undefined;
    live = false;
    privateLive = false;
    cancel("silence");
    cancel("poll");
    l?.close();
  }

  function reopen(): void {
    close();
    open();
  }

  /** The link failed or ended: reconnect with backoff, or fall back to the next transport. */
  function dropped(wait = 0): void {
    if (live) {
      report("state_disconnected", "the realtime connection dropped; reconnecting");
    }
    close();
    failures += 1;
    if (!worked.has(transport) && failures >= FALLBACK_AFTER) {
      const next = order[order.indexOf(transport) + 1];
      if (next !== undefined) {
        transport = next;
        failures = 0;
        return open();
      }
    }
    const backoff = Math.min(BACKOFF_MS[1], BACKOFF_MS[0] * 2 ** (failures - 1));
    later("retry", Math.max(wait, backoff * (0.5 + P.random() / 2)), open);
  }

  /** Realtime ends for this page (CLOSE 1, 2, 4, 5). */
  function halt(why: string): void {
    halted = true;
    close();
    report("state_disconnected", `realtime updates stopped: ${why}`);
  }

  function connected(): void {
    live = true;
    failures = 0;
    worked.add(transport);
    run(refreshMine());
  }

  /** One server frame; `pongs` answers PING; `withToken`: the SUBSCRIBE carried the hold token. */
  function frame(
    bytes: Uint8Array,
    pongs?: (b: Uint8Array<ArrayBuffer>) => void,
    withToken = false,
  ): void {
    try {
      switch (bytes[0]) {
        case HELLO: {
          const hello = readHello(bytes);
          if (hello.version !== 1) {
            return halt(`protocol version ${hello.version} is not supported`);
          }
          clock(hello.time - P.now());
          if (chartHash !== undefined && hello.hash !== chartHash) {
            report(
              "state_failed",
              `the event's chart is now ${hello.hash}, not the one loaded (${chartHash}): reload the page`,
            );
          }
          return connected();
        }
        case SNAPSHOT:
        case DELTA: {
          const [from, to] = readSeq(bytes);
          // A delta out of order changes nothing: the engine asks for a resync.
          if (bytes[0] === SNAPSHOT || from === seq) {
            seq = to;
          }
          return ctx.applyFrame(bytes);
        }
        case PRIVATE:
          return ctx.applyFrame(bytes);
        case PING:
          return pongs?.(pong(bytes));
        case CLOSE:
          return closed(bytes[1] ?? 3, withToken);
      }
      // RESYNC (the SNAPSHOT follows) and unknown frames: nothing to do.
    } catch (error) {
      report("state_failed", `realtime: ${String(error)}`);
      dropped();
    }
  }

  /** CLOSE (08 §8.6): reconnect for 3, 6 and unknown reasons; otherwise stop. */
  function closed(reason: number, withToken: boolean): void {
    if (reason === 5 && withToken) {
      // The token may not be followed: it has ended. Subscribe without it.
      close();
      ended();
      return open();
    }
    const stops: Record<number, string> = {
      1: "the event is archived",
      2: "the event moved to another chart version: reload the page",
      4: "the server does not speak this protocol version",
      5: "the server knows no such event for this key",
    };
    const why = stops[reason];
    return why === undefined ? dropped() : halt(why);
  }

  function openWs(): void {
    const Ws = P.WebSocket as typeof WebSocket;
    const sent = token;
    privateLive = sent !== null;
    const ws = new Ws(`${root.replace(/^http/, "ws")}/v1/public/rt?${keyParam}`);
    const off = new AbortController();
    const on = { signal: off.signal };
    const l = {
      close(): void {
        off.abort();
        ws.close();
      },
    };
    link = l;
    ws.binaryType = "arraybuffer";
    ws.addEventListener("open", () => ws.send(subscribe(event, seq, sent, session)), on);
    ws.addEventListener(
      "message",
      (message: MessageEvent<unknown>) => {
        later("silence", SILENCE_MS, () => dropped());
        if (message.data instanceof ArrayBuffer) {
          frame(new Uint8Array(message.data), (b) => ws.send(b), sent !== null);
        } else {
          report("state_failed", "realtime: a text message");
          dropped();
        }
      },
      on,
    );
    ws.addEventListener("close", () => dropped(), on);
    // Chromium fires only `error`, never `close`, for a socket a Content
    // Security Policy refuses; `dropped` closes the link, so one of the two counts.
    ws.addEventListener("error", () => dropped(), on);
    later("silence", SILENCE_MS, () => dropped());
  }

  function openSse(): void {
    const Es = P.EventSource as typeof EventSource;
    // Resumes from `seq`; `EventSource` never retries by itself here, so the backoff has jitter.
    const es = new Es(`${eventUrl}/stream?${keyParam}&last_seq=${seq}`);
    const off = new AbortController();
    const on = { signal: off.signal };
    link = {
      close(): void {
        off.abort();
        es.close();
      },
    };
    es.addEventListener(
      "message",
      (message: MessageEvent<string>) => {
        let bytes: Uint8Array;
        try {
          bytes = fromBase64(message.data);
        } catch {
          report("state_failed", "realtime: an SSE event that is not base64");
          return dropped();
        }
        frame(bytes);
      },
      on,
    );
    es.addEventListener("error", () => dropped(), on);
  }

  function openPoll(): void {
    const l = { close: () => cancel("poll") };
    link = l;
    const tick = async (): Promise<void> => {
      const res = await request(`${eventUrl}/state?${keyParam}&after_seq=${seq}`, {}, false).catch(
        () => undefined,
      );
      if (link !== l) {
        return;
      }
      if (res?.status !== 200 && res?.status !== 204) {
        return dropped(Number(res?.headers.get("Retry-After") ?? 0) * 1000 || 0);
      }
      const bytes = res.status === 200 ? new Uint8Array(await res.arrayBuffer()) : undefined;
      if (link !== l) {
        return;
      }
      if (!live) {
        // `Date` is readable on a same-origin server only; without it the page's clock is trusted.
        const date = Date.parse(res.headers.get("Date") ?? "");
        if (offset === undefined && Number.isFinite(date)) {
          clock(date + 500 - P.now());
        }
        connected();
      }
      if (bytes !== undefined) {
        frame(bytes);
      }
      if (link === l) {
        later("poll", POLL_MS, () => run(tick()));
      }
    };
    run(tick());
  }

  async function snapshot(): Promise<Uint8Array> {
    const res = await request(`${eventUrl}/state?${keyParam}`, {}, false);
    if (!res.ok) {
      const error = new Error(`snapshot of "${event}": ${said(res, await problem(res))}`);
      throw Object.assign(error, { status: res.status });
    }
    const bytes = new Uint8Array(await res.arrayBuffer());
    if (bytes[0] !== SNAPSHOT) {
      throw new Error(`snapshot of "${event}": not a SNAPSHOT frame`);
    }
    return bytes;
  }

  const start = (s: AbortSignal): void => {
    signal = s;
    P ??= platform();
  };

  const adapter: StateAdapter = {
    async chart(s) {
      start(s);
      return { url: `${root}${(await eventInfo()).chart.url}` };
    },
    image: (hash) => `${root}/v1/public/assets/${hash}`,
    async connect(c) {
      ctx = c;
      start(c.signal);
      const wanted: Array<"ws" | "sse" | "poll"> =
        options.transport === undefined || options.transport === "auto"
          ? ["ws", "sse", "poll"]
          : [options.transport];
      order = wanted.filter((t) => t === "poll" || (t === "ws" ? P.WebSocket : P.EventSource));
      transport = order[0] ?? "poll";
      const stored =
        mode === "continue" ? (options.holdToken ?? null) : mode === "start" ? read() : null;
      const [eventData, bytes] = await Promise.all([
        eventInfo(),
        snapshot(),
        stored === null ? undefined : restore(stored),
      ]);
      if (eventData.require_session && mode !== "manual" && mode !== "none") {
        if (options.sessionToken === undefined) {
          throw new Error(
            `event "${event}" requires a session token: pass sessionToken to caveaServer`,
          );
        }
      }
      chartHash = eventData.chart.hash;
      licence = eventData.licence;
      seq = readSeq(bytes)[1];
      open();
      return { kind: "binary", bytes };
    },
    resync() {
      // A SNAPSHOT on the stream itself (or the next poll), so it lines up with the deltas after it.
      seq = 0;
      if (link !== undefined) {
        reopen();
      }
    },
    pause() {
      if (options.pauseWhenHidden !== false) {
        later("pause", PAUSE_MS, () => {
          paused = true;
          close();
          cancel("retry");
        });
      }
    },
    resume() {
      cancel("pause");
      if (paused) {
        paused = false;
        failures = 0;
        open();
      }
    },
    disconnect() {
      stopped = true;
      close();
      for (const name of timers.keys()) {
        cancel(name);
      }
    },
  };
  if (mode === "start" || mode === "continue") {
    adapter.hold = hold;
    adapter.release = release;
  }
  // The map's licence mark follows the server's answer (12 §7.4).
  return reportLicence(adapter, () => fromServer(licence));
}
