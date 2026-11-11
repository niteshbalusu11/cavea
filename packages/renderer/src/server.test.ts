// `caveaServer` against scripted fakes of `fetch`, `WebSocket`, `EventSource`,
// storage and a virtual clock. The frames come from `wire.fixture.json`, which
// `crates/cavea-web` checks against the Rust encoders.
import { afterEach, describe, expect, test } from "bun:test";
import type { AdapterContext, HoldResult } from "./adapter.ts";
import { CaveaError } from "./errors.ts";
import { serverAdapter, type CaveaServerOptions, type Platform } from "./server.ts";
import fixture from "./wire.fixture.json";
import { pong, readHello, readSeq, subscribe } from "./wire.ts";

const hex = (b: ArrayBuffer | Uint8Array): string => Buffer.from(b as Uint8Array).toString("hex");
const frame = (name: keyof typeof fixture): Uint8Array<ArrayBuffer> =>
  new Uint8Array(Buffer.from(fixture[name], "hex"));
const varint = (n: number): number[] => {
  const out: number[] = [];
  for (; n >= 0x80; n = Math.floor(n / 0x80)) {
    out.push((n % 0x80) | 0x80);
  }
  return [...out, n];
};
/** A DELTA without changes: what matters here is its `seq`. */
const delta = (from: number, to: number): Uint8Array<ArrayBuffer> =>
  Uint8Array.from([3, ...varint(from), ...varint(to), 0, 0]);
const close = (reason: number): Uint8Array => Uint8Array.of(7, reason);
const PRIVATE = Uint8Array.of(4, 0, 0, 0);
const SEQ = 48_211;
const HASH = "9f2c4e1a7b3d5f6e8a0c2b4d6f8e0a1c";
const BASE = "https://seats.example/cavea";
// Fakes in the shapes of a publishable key, a hold token and a session token.
const KEY = `pk_${"0".repeat(64)}`;
const TOKEN = `ht_${"ab".repeat(32)}`;
const SESSION = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln";
const T0 = Date.UTC(2026, 10, 11, 12);
const iso = (ms: number): string => new Date(ms).toISOString();

const json = (status: number, body: unknown, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), { status, headers });
const refused = (status: number, code: string, details = {}, headers = {}): Response =>
  json(status, { code, status, message: code, details, request_id: "req_1" }, headers);

interface Req {
  method: string;
  path: string;
  url: string;
  headers: Record<string, string>;
  body: Record<string, unknown> | undefined;
}

/** Every URL a world built, checked after each test: no token in any. */
const urls: string[] = [];
afterEach(() => {
  for (const url of urls.splice(0)) {
    expect(url).not.toContain(TOKEN);
    expect(url).not.toContain(SESSION);
    expect(url.includes(KEY)).toBe(/\/(state|stream|rt)\?/.test(url));
  }
});

const settle = async (): Promise<void> => {
  for (let i = 0; i < 8; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
};

/** The server, the platform and the map's context, scripted; time is virtual. */
function world(options: Partial<CaveaServerOptions> = {}, store = new Map<string, string>()) {
  let now = T0;
  let ids = 0;
  const due = new Map<number, { at: number; run: () => void }>();
  const requests: Req[] = [];
  const log: string[] = [];
  const frames: string[] = [];
  const errors: string[] = [];
  const randoms: number[] = [];
  const state = { storageFails: false, held: [] as string[] };
  const routes: Record<string, (req: Req) => Response | Promise<Response>> = {
    "GET /events/hamlet": () =>
      json(200, {
        event: "hamlet",
        chart: { hash: HASH, url: `/v1/public/charts/${HASH}.ccf` },
        hold_ttl_seconds: 900,
        max_objects: 10,
        require_session: false,
      }),
    "GET /events/hamlet/state": () => new Response(frame("snapshot")),
    "GET /hold-tokens/current": (req) =>
      req.headers["cavea-hold-token"] === TOKEN
        ? json(200, {
            event: "hamlet",
            expires_at: iso(T0 + 900_000),
            max_objects: 10,
            objects: state.held.map((label) => ({ label, oid: 1 })),
          })
        : refused(404, "not_found", { missing: "hold_token" }),
    "POST /hold-tokens": () =>
      json(201, { hold_token: TOKEN, event: "hamlet", seq: 1, expires_at: iso(now + 900_000) }),
    "POST /events/hamlet/hold": (req) => {
      state.held.push(...(req.body?.["objects"] as string[]));
      return json(200, { seq: 2, expires_at: iso(T0 + 900_000), objects: [] });
    },
    "POST /events/hamlet/release": (req) => {
      const released = req.body?.["objects"] as string[];
      state.held = state.held.filter((l) => !released.includes(l));
      return json(200, { seq: 3, objects: [] });
    },
  };
  class Socket extends EventTarget {
    binaryType = "blob";
    sent: string[] = [];
    closed = false;
    constructor(readonly url: string) {
      super();
      urls.push(url);
      sockets.push(this);
    }
    send(b: Uint8Array): void {
      this.sent.push(hex(b));
    }
    close(): void {
      this.closed = true;
    }
    open(): void {
      this.dispatchEvent(new Event("open"));
    }
    recv(...data: Uint8Array[]): void {
      for (const b of data) {
        this.dispatchEvent(new MessageEvent("message", { data: b.slice().buffer }));
      }
    }
    drop(): void {
      this.dispatchEvent(new Event("close"));
    }
  }
  class Source extends EventTarget {
    closed = false;
    constructor(readonly url: string) {
      super();
      urls.push(url);
      sources.push(this);
    }
    close(): void {
      this.closed = true;
    }
    recv(...data: Uint8Array[]): void {
      for (const b of data) {
        this.dispatchEvent(
          new MessageEvent("message", { data: Buffer.from(b).toString("base64") }),
        );
      }
    }
    fail(): void {
      this.dispatchEvent(new Event("error"));
    }
  }
  const sockets: Socket[] = [];
  const sources: Source[] = [];
  const platform: Platform = {
    async fetch(url, init) {
      urls.push(url);
      const u = new URL(url);
      const body = init.body === undefined ? undefined : JSON.parse(String(init.body));
      const headers = Object.fromEntries(new Headers(init.headers));
      const path = u.pathname.replace("/cavea/v1/public", "");
      const req: Req = { method: init.method ?? "GET", path, url, headers, body };
      requests.push(req);
      const aborted = new Promise<never>((_, reject) =>
        init.signal?.addEventListener("abort", () =>
          reject(new DOMException("aborted", "AbortError")),
        ),
      );
      const answer = routes[`${req.method} ${path}`]?.(req) ?? new Response(null, { status: 404 });
      return Promise.race([answer, aborted]);
    },
    WebSocket: Socket as unknown as typeof WebSocket,
    EventSource: Source as unknown as typeof EventSource,
    storage: () => {
      if (state.storageFails) {
        throw new DOMException("denied", "SecurityError");
      }
      return {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => store.set(k, v),
        removeItem: (k: string) => store.delete(k),
      } as unknown as Storage;
    },
    setTimeout(run, ms) {
      ids += 1;
      due.set(ids, { at: now + ms, run });
      return ids;
    },
    clearTimeout: (id) => due.delete(id as number),
    now: () => now,
    random: () => randoms.shift() ?? 1,
  };
  const abort = new AbortController();
  const ctx: AdapterContext = {
    signal: abort.signal,
    applyFrame: (b) => frames.push(hex(b)),
    applySnapshot: () => log.push("snapshot"),
    applyDelta: () => log.push("delta"),
    setMine: (labels, at) => log.push(`mine [${labels.join()}]${at ? ` ${iso(at)}` : ""}`),
    setHoldToken: (t) => log.push(`token ${t === TOKEN ? "T" : t}`),
    setClockOffset: (ms) => log.push(`offset ${ms}`),
    reportError: (e) =>
      errors.push(e instanceof CaveaError ? `${e.code}: ${e.message}` : String(e)),
  };
  const adapter = serverAdapter(
    { baseUrl: `${BASE}/`, publishableKey: KEY, event: "hamlet", ...options },
    () => platform,
  );
  /** Moves the clock, running timers as they fall due. */
  async function advance(ms: number): Promise<void> {
    const end = now + ms;
    for (;;) {
      await settle();
      let next: [number, { at: number; run: () => void }] | undefined;
      for (const entry of due) {
        next = next === undefined || entry[1].at < next[1].at ? entry : next;
      }
      if (next === undefined || next[1].at > end) {
        break;
      }
      due.delete(next[0]);
      now = Math.max(now, next[1].at);
      next[1].run();
    }
    now = end;
    await settle();
  }
  const paths = (): string[] => requests.map((r) => `${r.method} ${r.path}`);
  return {
    adapter,
    platform,
    ctx,
    abort,
    routes,
    requests,
    paths,
    sockets,
    sources,
    store,
    state,
    log,
    frames,
    errors,
    randoms,
    due,
    advance,
    settle,
    now: () => now,
    socket: (): Socket => sockets.at(-1) as Socket,
    source: (): Source => sources.at(-1) as Source,
    /** Connects, and opens the WebSocket with its HELLO. */
    async live(): Promise<void> {
      await adapter.connect(ctx);
      sockets.at(-1)?.open();
      sockets.at(-1)?.recv(frame("hello"));
      await settle();
    },
  };
}

test("frames match the Rust encoders", () => {
  expect(hex(subscribe("hamlet-2027-03-14", SEQ, TOKEN, SESSION))).toBe(fixture.subscribe);
  expect(hex(subscribe("Bühne", 0, null, null))).toBe(fixture.subscribeBare);
  expect(hex(pong(frame("ping")))).toBe(fixture.pong);
  expect(readHello(frame("hello"))).toEqual({ version: 1, time: 1_793_000_000_123, hash: HASH });
  expect(readSeq(frame("snapshot"))).toEqual([SEQ, SEQ]);
  expect(readSeq(frame("delta"))).toEqual([SEQ, SEQ + 1]);
  expect(() => readHello(frame("hello").subarray(0, 20))).toThrow();
});

describe("connect", () => {
  test("event info, snapshot and the stored token's holds are requested together; then the stream", async () => {
    const w = world();
    w.store.set(`cavea:hold:${BASE}|${KEY}|hamlet`, TOKEN);
    w.state.held = ["A-1"];
    const chart = w.adapter.chart?.(w.ctx.signal);
    const connected = w.adapter.connect(w.ctx);
    expect(w.paths()).toEqual([
      "GET /events/hamlet",
      "GET /events/hamlet/state",
      "GET /hold-tokens/current",
    ]);
    expect(w.requests[0]?.headers["authorization"]).toBe(`Bearer ${KEY}`);
    expect(w.requests[1]?.headers["authorization"]).toBeUndefined();
    expect(await chart).toEqual({ url: `${BASE}/v1/public/charts/${HASH}.ccf` });
    expect(await connected).toEqual({ kind: "binary", bytes: frame("snapshot") });
    // Its holds come in the PRIVATE frame the WebSocket gets for the token.
    expect(w.log).toEqual(["token T"]);

    const ws = w.socket();
    expect(ws.url).toBe(`wss://seats.example/cavea/v1/public/rt?key=${KEY}`);
    ws.open();
    expect(ws.sent).toEqual([hex(subscribe("hamlet", SEQ, TOKEN, null))]);
    ws.recv(
      frame("hello"),
      frame("delta"),
      PRIVATE,
      Uint8Array.of(5),
      frame("ping"),
      Uint8Array.of(99),
    );
    expect(w.log.at(-1)).toBe(`offset ${1_793_000_000_123 - T0}`);
    expect(w.frames).toEqual([fixture.delta, hex(PRIVATE)]);
    expect(ws.sent.at(-1)).toBe(fixture.pong);
    // HELLO naming another chart: the event moved to a new version.
    ws.recv(Uint8Array.from([...frame("hello").subarray(0, 9), 0, ...frame("hello").subarray(10)]));
    expect(w.errors).toEqual([expect.stringContaining("state_failed: the event's chart is now")]);
  });

  test("CLOSE: reconnect for 3, 6 and unknown reasons; stop for 1, 2, 4, and 5 without a token", async () => {
    for (const [reason, reconnects] of [
      [1, false],
      [2, false],
      [3, true],
      [4, false],
      [5, false],
      [6, true],
      [9, true],
    ] as const) {
      const w = world();
      await w.live();
      w.socket().recv(close(reason));
      await w.advance(60_000);
      expect([reason, w.sockets.length]).toEqual([reason, reconnects ? 2 : 1]);
      expect(w.errors).toEqual([
        expect.stringContaining(reconnects ? "dropped; reconnecting" : "realtime updates stopped"),
      ]);
    }
  });

  test("CLOSE 5 after a SUBSCRIBE with the token: the session ended; subscribe again without it", async () => {
    const w = world();
    w.store.set(`cavea:hold:${BASE}|${KEY}|hamlet`, TOKEN);
    await w.live();
    w.socket().recv(close(5));
    await w.settle();
    expect(w.log.slice(-2)).toEqual(["token null", "mine []"]);
    expect(w.store.size).toBe(0);
    w.socket().open();
    expect(w.socket().sent).toEqual([hex(subscribe("hamlet", SEQ, null, null))]);
  });
});

describe("reconnection", () => {
  test("backoff doubles to a cap, with jitter, resumes from the current seq, and stops on disconnect", async () => {
    const w = world({ transport: "ws" });
    await w.live();
    w.socket().recv(frame("delta"));
    w.randoms.push(1, 0, 1, 0, 1, 1, 1, 1, 1, 1);
    const delays: number[] = [];
    for (let i = 0; i < 10; i++) {
      w.socket().drop();
      // The retry is the only timer left.
      const [retry] = [...w.due.values()];
      delays.push((retry?.at ?? 0) - w.now());
      await w.advance(delays.at(-1) ?? 0);
    }
    // 500 ms doubling to 30 s, each times 0.5 to 1 (here 1 or 0.5).
    expect(delays).toEqual([500, 500, 2000, 2000, 8000, 16000, 30000, 30000, 30000, 30000]);
    w.socket().open();
    expect(w.socket().sent).toEqual([hex(subscribe("hamlet", SEQ + 1, null, null))]);

    w.socket().drop();
    w.adapter.disconnect();
    expect(w.due.size).toBe(0);
    await w.advance(120_000);
    expect(w.sockets.length).toBe(11);
  });

  test("auto falls back after two failures with no HELLO: WebSocket, SSE, polling; a transport that worked is kept", async () => {
    const w = world();
    await w.adapter.connect(w.ctx);
    w.socket().drop();
    await w.advance(1_000);
    w.socket().drop();
    await w.settle();
    expect(w.source().url).toBe(
      `${BASE}/v1/public/events/hamlet/stream?key=${KEY}&last_seq=${SEQ}`,
    );
    w.source().fail();
    await w.advance(1_000);
    w.source().fail();
    await w.settle();
    expect([w.sockets.length, w.sources.length]).toEqual([2, 2]);
    expect(w.requests.at(-1)?.url).toBe(
      `${BASE}/v1/public/events/hamlet/state?key=${KEY}&after_seq=${SEQ}`,
    );

    const kept = world();
    await kept.live();
    for (let i = 0; i < 5; i++) {
      kept.socket().drop();
      await kept.advance(60_000);
    }
    expect([kept.sockets.length, kept.sources.length]).toEqual([6, 0]);
  });

  test("a forced transport is the only one tried", async () => {
    for (const transport of ["sse", "poll"] as const) {
      const w = world({ transport });
      await w.adapter.connect(w.ctx);
      await w.advance(10_000);
      expect([w.sockets.length, w.sources.length > 0]).toEqual([0, transport === "sse"]);
    }
  });

  test("one server history gives the engine the same frames in order on every transport", async () => {
    const history = [delta(SEQ, SEQ + 1), delta(SEQ + 1, SEQ + 2)];
    const seen: string[][] = [];
    for (const transport of ["ws", "sse", "poll"] as const) {
      const w = world({ transport });
      const polls = [history[0], undefined, history[1]];
      w.routes["GET /events/hamlet/state"] = (req) =>
        !req.url.includes("after_seq")
          ? new Response(frame("snapshot"))
          : polls.length > 0 && polls[0] === undefined
            ? (polls.shift(), new Response(null, { status: 204 }))
            : polls.length > 0
              ? new Response(polls.shift())
              : new Response(null, { status: 204 });
      await w.adapter.connect(w.ctx);
      if (transport === "ws") {
        w.socket().open();
        w.socket().recv(frame("hello"), ...history);
      } else if (transport === "sse") {
        w.source().recv(frame("hello"), ...history);
      } else {
        await w.advance(6_000);
        expect(
          w.requests.filter((r) => r.url.includes("after_seq")).map((r) => r.url.split("=").at(-1)),
        ).toEqual([`${SEQ}`, `${SEQ + 1}`, `${SEQ + 1}`, `${SEQ + 2}`]);
      }
      seen.push(w.frames);
    }
    expect(seen[1]).toEqual(seen[0] ?? []);
    expect(seen[2]).toEqual(seen[0] ?? []);
    expect(seen[0]?.length).toBe(2);
  });

  test("a resync asks the stream for a SNAPSHOT; hidden pages drop it after a grace and resume from seq", async () => {
    const w = world();
    await w.live();
    w.socket().recv(frame("delta"));
    w.adapter.resync?.();
    w.socket().open();
    expect(w.socket().sent).toEqual([hex(subscribe("hamlet", 0, null, null))]);
    w.socket().recv(frame("snapshot"));

    w.adapter.pause?.();
    await w.advance(5_000);
    w.adapter.resume?.();
    expect(w.sockets.length).toBe(2);
    w.adapter.pause?.();
    await w.advance(20_000);
    expect(w.socket().closed).toBe(true);
    await w.advance(60_000);
    w.adapter.resume?.();
    w.socket().open();
    expect([w.sockets.length, w.socket().sent]).toEqual([
      3,
      [hex(subscribe("hamlet", SEQ, null, null))],
    ]);
    expect(w.errors).toEqual([]);

    const kept = world({ pauseWhenHidden: false });
    await kept.live();
    kept.adapter.pause?.();
    await kept.advance(30_000);
    expect(kept.socket().closed).toBe(false);
  });
});

const hold = (w: ReturnType<typeof world>, ...labels: string[]): Promise<HoldResult> =>
  w.adapter.hold?.(labels) ?? Promise.reject(new Error("no hold"));

describe("holds", () => {
  test("the first hold creates the token, stores it and subscribes again with it for PRIVATE frames", async () => {
    const w = world();
    await w.live();
    expect(await hold(w, "A-1", "A-2")).toEqual({ ok: true });
    expect(w.paths().slice(2)).toEqual(["POST /hold-tokens", "POST /events/hamlet/hold"]);
    expect(w.requests.at(-1)?.body).toEqual({ hold_token: TOKEN, objects: ["A-1", "A-2"] });
    expect(w.store.get(`cavea:hold:${BASE}|${KEY}|hamlet`)).toBe(TOKEN);
    expect(w.log).toContain("token T");
    w.socket().open();
    expect(w.socket().sent).toEqual([hex(subscribe("hamlet", SEQ, TOKEN, null))]);
    w.socket().recv(frame("hello"));
    await w.adapter.release?.(["A-1"]);
    await w.settle();
    // PRIVATE frames carry the holds: nothing reads them over HTTP.
    expect(w.paths().slice(4)).toEqual(["POST /events/hamlet/release"]);
  });

  test("refusals map to codes and per-object reasons; a token gone ends the session", async () => {
    const w = world();
    await w.live();
    const answers = [
      refused(409, "object_not_free", { objects: [{ label: "A-2", oid: 2, reason: "not_free" }] }),
      refused(422, "hold_limit_exceeded", { limit: 2, requested: 3 }),
      refused(404, "not_found", { missing: "objects", objects: [{ label: "A-9" }] }),
      refused(400, "invalid_request", { reason: "invalid_field" }),
      refused(404, "not_found", { missing: "hold_token" }),
    ];
    w.routes["POST /events/hamlet/hold"] = () => answers.shift() ?? json(200, {});
    expect(await hold(w, "A-1", "A-2")).toEqual({
      ok: false,
      code: "object_not_free",
      failed: [{ label: "A-2", reason: "not_free" }],
    });
    expect(await hold(w, "A-1")).toEqual({ ok: false, code: "hold_limit_exceeded" });
    expect(await hold(w, "A-9")).toEqual({
      ok: false,
      code: "unknown_object",
      failed: [{ label: "A-9", reason: "unknown_object" }],
    });
    expect(await hold(w, "A-1")).toEqual({ ok: false, code: "invalid_request" });
    expect(w.errors).toEqual([
      "hold_failed: hold refused: not_found (HTTP 404)",
      "hold_failed: hold refused: invalid_request (HTTP 400)",
    ]);
    // The token has ended (expired, or booked): its holds leave, and the hold is sent again with a new one.
    expect(await hold(w, "A-1")).toEqual({ ok: true });
    expect(w.log.slice(-3)).toEqual(["token null", "mine []", "token T"]);
    expect(w.paths().filter((p) => p === "POST /hold-tokens")).toHaveLength(2);
    expect(w.paths().slice(-3)).toEqual([
      "POST /events/hamlet/hold",
      "POST /hold-tokens",
      "POST /events/hamlet/hold",
    ]);
  });

  test("429 and 503 are retried once when Retry-After is short; network failures once; never longer", async () => {
    const w = world();
    await w.live();
    let failures = [
      () => refused(429, "rate_limited", {}, { "Retry-After": "1" }),
      () => json(200, {}),
      () => refused(503, "overloaded", {}, { "Retry-After": "30" }),
    ];
    w.routes["POST /events/hamlet/hold"] = () => failures.shift()?.() ?? json(200, {});
    const first = hold(w, "A-1");
    await w.advance(999);
    expect(w.paths().filter((p) => p.endsWith("/hold"))).toHaveLength(1);
    await w.advance(1);
    expect(await first).toEqual({ ok: true });
    expect(await hold(w, "A-1")).toEqual({ ok: false, code: "overloaded" });
    expect(w.paths().filter((p) => p.endsWith("/hold"))).toHaveLength(3);

    failures = [
      () => {
        throw new TypeError("Failed to fetch");
      },
      () => {
        throw new TypeError("Failed to fetch");
      },
    ];
    const lost = hold(w, "A-3").catch((e: unknown) => String(e));
    await w.advance(500);
    expect(await lost).toBe("TypeError: Failed to fetch");
    expect(w.paths().filter((p) => p.endsWith("/hold"))).toHaveLength(5);
  });

  test("a hold gives up 18 s after it started, token creation and retries included", async () => {
    const w = world();
    await w.live();
    w.routes["POST /hold-tokens"] = () =>
      new Promise((r) => w.platform.setTimeout(() => r(json(201, { hold_token: TOKEN })), 7_000));
    w.routes["POST /events/hamlet/hold"] = () => new Promise(() => {});
    let settled = false;
    const result = hold(w, "A-1").catch((e: unknown) => String(e));
    void result.then(() => (settled = true));
    await w.advance(17_999);
    expect(settled).toBe(false);
    await w.advance(1);
    expect(await result).toContain("AbortError");
    // Aborted at 15 s (8 s after the token), retried at 15.5 s and aborted at 18 s.
    expect(w.paths().filter((p) => p.endsWith("/hold"))).toHaveLength(2);
  });

  test("a failed read of the holds is tried again, even before any hold is known", async () => {
    const w = world({ transport: "poll" });
    w.store.set(`cavea:hold:${BASE}|${KEY}|hamlet`, TOKEN);
    w.state.held = ["A-1"];
    const current = w.routes["GET /hold-tokens/current"];
    let failures = 2;
    w.routes["GET /hold-tokens/current"] = (req) =>
      failures-- > 0 ? refused(503, "unavailable") : (current?.(req) ?? refused(404, "not_found"));
    await w.adapter.connect(w.ctx);
    await w.advance(10_000);
    expect(w.log).toContain(`mine [A-1] ${iso(T0 + 900_000)}`);
  });

  test("nothing runs after disconnect, whatever answers late", async () => {
    const w = world({ transport: "sse" });
    await w.adapter.connect(w.ctx);
    w.source().recv(frame("hello"));
    let answer!: (r: Response) => void;
    w.routes["POST /events/hamlet/hold"] = () => new Promise((r) => (answer = r));
    const pending = hold(w, "A-1").catch((e: unknown) => String(e));
    await w.settle();
    const sent = w.requests.length;
    w.adapter.disconnect();
    answer(json(200, {}));
    await pending;
    await w.advance(120_000);
    expect([w.due.size, w.requests.length, w.sources.length]).toEqual([0, sent, 1]);
  });

  test("on SSE and polling the buyer's holds come from hold-tokens/current, kept in order with replies", async () => {
    const w = world({ transport: "sse" });
    await w.adapter.connect(w.ctx);
    w.source().recv(frame("hello"));
    expect(await hold(w, "A-1")).toEqual({ ok: true });
    await w.settle();
    expect(w.log.slice(-1)).toEqual([`mine [A-1] ${iso(T0 + 900_000)}`]);

    // A read the next hold overtakes is dropped; the hold's own read follows.
    let answer!: (r: Response) => void;
    w.routes["GET /hold-tokens/current"] = () => new Promise((r) => (answer = r));
    await w.adapter.release?.(["A-1"]);
    const slow = answer;
    w.routes["GET /hold-tokens/current"] = () =>
      json(200, { event: "hamlet", expires_at: iso(T0 + 900_000), objects: [{ label: "A-2" }] });
    expect(await hold(w, "A-2")).toEqual({ ok: true });
    await w.settle();
    slow(json(200, { event: "hamlet", expires_at: iso(T0 + 900_000), objects: [] }));
    await w.settle();
    expect(w.log.slice(-1)).toEqual([`mine [A-2] ${iso(T0 + 900_000)}`]);

    // At the expiry the holds are read again: the token is gone.
    w.routes["GET /hold-tokens/current"] = () =>
      refused(404, "not_found", { missing: "hold_token" });
    await w.advance(900_000);
    expect(w.log.slice(-2)).toEqual(["token null", "mine []"]);
  });
});

describe("sessions", () => {
  test("manual and none hold nothing and read no token", async () => {
    for (const session of ["manual", "none"] as const) {
      const w = world({ session });
      w.store.set(`cavea:hold:${BASE}|${KEY}|hamlet`, TOKEN);
      await w.adapter.connect(w.ctx);
      expect([w.adapter.hold, w.adapter.release, w.paths().length, w.log]).toEqual([
        undefined,
        undefined,
        2,
        [],
      ]);
    }
  });

  test("continue uses the given token, stores nothing, and reports when it has ended", async () => {
    const w = world({ session: "continue", holdToken: TOKEN });
    await w.live();
    expect(w.log[0]).toBe("token T");
    w.routes["POST /events/hamlet/hold"] = () =>
      refused(404, "not_found", { missing: "hold_token" });
    expect(await w.adapter.hold?.(["A-1"])).toEqual({ ok: false, code: "not_found" });
    expect(await w.adapter.hold?.(["A-1"])).toEqual({ ok: false, code: "session_ended" });
    expect([w.store.size, w.errors]).toEqual([
      0,
      [expect.stringContaining("hold token has ended")],
    ]);
  });

  test("session tokens: required early, sent as Cavea-Session and in SUBSCRIBE, refreshed once when expired", async () => {
    const required = world();
    required.routes["GET /events/hamlet"] = () =>
      json(200, {
        chart: { hash: HASH, url: "/c.ccf" },
        hold_ttl_seconds: 900,
        require_session: true,
      });
    expect(required.adapter.connect(required.ctx)).rejects.toThrow("requires a session token");

    const fixed = world({ sessionToken: SESSION });
    await fixed.live();
    expect(fixed.sockets[0]?.sent).toEqual([hex(subscribe("hamlet", SEQ, null, SESSION))]);

    let calls = 0;
    const w = world({ sessionToken: () => Promise.resolve(`${SESSION}${(calls += 1)}`) });
    await w.live();
    expect(calls).toBe(0);
    let expired = true;
    w.routes["POST /events/hamlet/hold"] = () =>
      expired
        ? ((expired = false),
          refused(401, "unauthorized", { reason: "session_token", session: "expired" }))
        : json(200, {});
    expect(await w.adapter.hold?.(["A-1"])).toEqual({ ok: true });
    expect(
      w.requests.filter((r) => r.method === "POST").map((r) => r.headers["cavea-session"]),
    ).toEqual([`${SESSION}1`, `${SESSION}1`, `${SESSION}2`]);
  });

  test("invalid options throw invalid_option naming the option", () => {
    const base = { baseUrl: BASE, publishableKey: KEY, event: "hamlet" };
    for (const [option, input] of [
      ["state.event", { ...base, event: "" }],
      ["state.baseUrl", { ...base, baseUrl: "nope" }],
      ["state.session", { ...base, session: "later" }],
      ["state.holdToken", { ...base, session: "continue" }],
      ["state.transport", { ...base, transport: "pigeon" }],
    ] as const) {
      expect(() => serverAdapter(input as CaveaServerOptions, () => ({}) as Platform)).toThrow(
        expect.objectContaining({ code: "invalid_option", option }),
      );
    }
  });
});

describe("persistence", () => {
  test("a stored token the server no longer knows is dropped silently", async () => {
    const w = world();
    w.store.set(`cavea:hold:${BASE}|${KEY}|hamlet`, `ht_${"cd".repeat(32)}`);
    await w.live();
    expect([w.store.size, w.log, w.errors]).toEqual([0, [`offset ${1_793_000_000_123 - T0}`], []]);
    expect(w.socket().sent).toEqual([hex(subscribe("hamlet", SEQ, null, null))]);
  });

  test("without sessionStorage the token lives in memory and still survives a new map", async () => {
    const first = world();
    first.state.storageFails = true;
    await first.live();
    await first.adapter.hold?.(["A-1"]);
    first.adapter.disconnect();

    const reload = world();
    reload.state.storageFails = true;
    reload.state.held = ["A-1"];
    await reload.live();
    expect(reload.log[0]).toBe("token T");
    expect(reload.socket().sent).toEqual([hex(subscribe("hamlet", SEQ, TOKEN, null))]);
    // Ended, so the next test starts clean.
    reload.socket().recv(close(5));
  });
});
