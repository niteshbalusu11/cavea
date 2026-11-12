// The client's rules against a scripted `fetch`: credentials, encoding,
// idempotency keys, which calls are retried and how long they wait, timeouts
// and cancellation, errors, pages and session tokens.
import { describe, expect, test } from "bun:test";
import { createHash, createHmac } from "node:crypto";
import { inspect } from "node:util";
import { Cavea, CaveaError, type CaveaOptions } from "./index.ts";

// Built from parts at run time, in the shapes of a secret key and its id.
const KEY = ["sk", "5e".repeat(32)].join("_");
const KEY_ID = ["key", "0f".repeat(16)].join("_");
const ADMIN = ["admin", "fake", "x".repeat(24)].join("-");
const TOKEN = ["ht", "ab".repeat(32)].join("_");
const BASE = "https://seats.example/cavea";

interface Sent {
  method: string;
  url: URL;
  headers: Headers;
  body: string | undefined;
}

type Script = (req: Sent, n: number) => Response | Promise<Response> | Error;

const json = (status: number, body: unknown, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });
const refused = (
  status: number,
  code: string,
  details: Record<string, unknown> = {},
  headers: Record<string, string> = {},
): Response =>
  new Response(
    JSON.stringify({
      code,
      status,
      message: `Failed to deserialize: invalid type: string "secret-order-123"`,
      details,
      request_id: "req_1",
    }),
    {
      status,
      headers: {
        "Content-Type": "application/problem+json",
        "Cavea-Request-Id": "req_1",
        ...headers,
      },
    },
  );
const SEAT = { seq: 7, objects: [{ label: "A-1", oid: 1 }] };

/** A client over `script`, and every request it sent. */
function world(script: Script, options: Partial<CaveaOptions> = {}) {
  const sent: Sent[] = [];
  const cavea = new Cavea({
    baseUrl: BASE,
    secretKey: KEY,
    keyId: KEY_ID,
    adminKey: ADMIN,
    maxRetryAfter: 2,
    fetch: async (input, init) => {
      const req: Sent = {
        method: init.method ?? "GET",
        url: new URL(input),
        headers: new Headers(init.headers),
        body: typeof init.body === "string" ? init.body : undefined,
      };
      sent.push(req);
      if (init.signal?.aborted) {
        throw init.signal.reason;
      }
      const out = await script(req, sent.length);
      if (out instanceof Error) {
        throw out;
      }
      return out;
    },
    ...options,
  });
  return { cavea, sent };
}

/** A `fetch` that answers only when its signal aborts. */
const hang = (req: Sent, init?: RequestInit): Promise<Response> =>
  new Promise((_, reject) =>
    init?.signal?.addEventListener("abort", () => reject(init.signal?.reason)),
  );

const network = (): Error => new TypeError("fetch failed", { cause: { code: "ECONNRESET" } });

const decode = (s: string | undefined): unknown =>
  JSON.parse(Buffer.from(s ?? "", "base64url").toString());

async function rejection(promise: Promise<unknown>): Promise<unknown> {
  try {
    await promise;
  } catch (e) {
    return e;
  }
  throw new Error("expected a rejection");
}

describe("credentials", () => {
  test("refuses a browser unless told otherwise", () => {
    const g = globalThis as Record<string, unknown>;
    g["window"] = {};
    g["document"] = {};
    try {
      expect(() => new Cavea({ baseUrl: BASE, secretKey: KEY })).toThrow(/browser/);
      expect(
        () => new Cavea({ baseUrl: BASE, secretKey: KEY, dangerouslyAllowBrowser: true }),
      ).not.toThrow();
    } finally {
      delete g["window"];
      delete g["document"];
    }
  });

  test("checks keys without printing them", () => {
    const pk = ["pk", "1".repeat(64)].join("_");
    for (const bad of [pk, "sk_short", `${KEY}0`]) {
      let message = "";
      try {
        expect(new Cavea({ baseUrl: BASE, secretKey: bad })).toBeUndefined();
      } catch (e) {
        message = String(e);
      }
      expect(message).toMatch(/secret key/);
      expect(message).not.toContain(bad);
    }
    // An admin key no header can carry is refused before `Headers` would
    // print it in its own error.
    for (const bad of [`${ADMIN}\nembedded`, `${ADMIN} spaced`, "short"]) {
      let message = "";
      try {
        expect(new Cavea({ baseUrl: BASE, adminKey: bad })).toBeUndefined();
      } catch (e) {
        message = String(e);
      }
      expect(message).toMatch(/adminKey must be/);
      expect(message).not.toContain(ADMIN);
    }
    expect(() => new Cavea({ baseUrl: "ftp://x", secretKey: KEY })).toThrow(/http/);
    expect(() => new Cavea({ baseUrl: `https://u:p@x`, secretKey: KEY })).toThrow(/credentials/);
    expect(() => new Cavea({ baseUrl: BASE })).toThrow(/secretKey/);
  });

  test("each operation's key in Authorization, never in the URL or a printed object", async () => {
    const { cavea, sent } = world((req) =>
      req.url.pathname.endsWith("/v1/workspaces")
        ? json(200, { data: [], next: null })
        : refused(404, "not_found", { missing: "hold_token" }),
    );
    for await (const _ of cavea.workspaces.list()) {
      // none
    }
    const error = await rejection(cavea.holdTokens.get(TOKEN));
    expect(sent[0]?.headers.get("Authorization")).toBe(`Bearer ${ADMIN}`);
    expect(sent[1]?.headers.get("Authorization")).toBe(`Bearer ${KEY}`);
    for (const printed of [
      inspect(cavea, { depth: 5, showHidden: true }),
      String(error),
      inspect(error, { depth: 5, showHidden: true }),
      JSON.stringify(error),
      ...sent.map((s) => s.url.href),
    ]) {
      expect(printed).not.toContain(KEY);
      expect(printed).not.toContain(ADMIN);
    }
    // The hold token is in the path (secret routes), never in the error.
    expect(inspect(error, { depth: 5 })).not.toContain(TOKEN);
    // Nor the server's message, which quoted the body.
    expect(String(error)).not.toContain("secret-order-123");
  });

  test("an operation without its key fails before sending", async () => {
    const { sent } = world(() => json(200, {}));
    const admin = new Cavea({ baseUrl: BASE, adminKey: ADMIN, fetch: async () => json(200, {}) });
    expect(String(await rejection(admin.charts.get("c")))).toMatch(/secretKey/);
    expect(sent).toHaveLength(0);
  });
});

describe("requests", () => {
  test("paths are encoded, queries repeat, the base's prefix is kept", async () => {
    const { cavea, sent } = world(() => json(200, { data: [], unknown: [] }));
    await cavea.events.objects("show/1 ?#", ["A 1", "B&2"]);
    const url = sent[0]?.url;
    expect(url?.pathname).toBe("/cavea/v1/events/show%2F1%20%3F%23/objects");
    expect(url?.searchParams.getAll("label")).toEqual(["A 1", "B&2"]);
    expect(sent[0]?.headers.get("Idempotency-Key")).toBeNull();
  });

  test("a request id of the right form is sent, another is not", async () => {
    const { cavea, sent } = world(() => json(200, {}));
    await cavea.charts.get("c", { requestId: "order-77.a_b" });
    await cavea.charts.get("c", { requestId: "has space" });
    expect(sent[0]?.headers.get("Cavea-Request-Id")).toBe("order-77.a_b");
    expect(sent[1]?.headers.get("Cavea-Request-Id")).toBeNull();
  });

  test("drafts: the etag out of ETag, If-Match in; publish says whether it created", async () => {
    const { cavea, sent } = world((req, n) =>
      n === 1
        ? json(200, { schema: 1 }, { ETag: '"4"' })
        : req.method === "PUT"
          ? json(200, { key: "c" })
          : json(n === 3 ? 201 : 200, { number: 1 }),
    );
    const { document, etag } = await cavea.charts.getDraft("c");
    await cavea.charts.saveDraft("c", document, { ifMatch: etag });
    expect((await cavea.charts.publish("c", { ifMatch: '"5"' })).created).toBe(true);
    expect((await cavea.charts.publish("c")).created).toBe(false);
    expect(sent[1]?.headers.get("If-Match")).toBe('"4"');
    expect(sent[1]?.body).toBe('{"schema":1}');
    expect(sent[2]?.headers.get("If-Match")).toBe('"5"');
    expect(sent[3]?.headers.get("If-Match")).toBeNull();
  });
});

describe("idempotency keys", () => {
  test("one per logical call, the same on each retry; the caller's when given", async () => {
    const { cavea, sent } = world(
      (_, n) => (n % 2 === 1 ? refused(503, "overloaded") : json(200, SEAT)),
      {
        maxRetryAfter: 0,
      },
    );
    await cavea.events.hold("e", { hold_token: TOKEN, objects: ["A-1"] });
    await cavea.events.book(
      "e",
      { hold_token: TOKEN, order_ref: "77" },
      { idempotencyKey: "order-77" },
    );
    const keys = sent.map((s) => s.headers.get("Idempotency-Key"));
    expect(keys[0]).toMatch(/^[0-9a-f-]{36}$/);
    expect(keys[1]).toBe(keys[0]);
    expect(keys[2]).toBe("order-77");
    expect(keys[3]).toBe("order-77");
  });

  test("a malformed key, or a key where none is taken, fails before sending", async () => {
    const { cavea, sent } = world(() => json(200, SEAT));
    expect(
      String(await rejection(cavea.events.book("e", { order_ref: "1" }, { idempotencyKey: "é" }))),
    ).toMatch(/printable/);
    // Empty, or a space at either end, which a header value would lose:
    // `"k "` would arrive as `"k"` and collide with it.
    for (const idempotencyKey of ["", " ", "order-77 ", " order-77", "x".repeat(256)]) {
      expect(
        String(await rejection(cavea.events.book("e", { order_ref: "1" }, { idempotencyKey }))),
      ).toMatch(/printable/);
    }
    expect(
      String(await rejection(cavea.holdTokens.create({ event: "e" }, { idempotencyKey: "k" }))),
    ).toMatch(/takes no idempotency key/);
    expect(sent).toHaveLength(0);
  });
});

describe("retries", () => {
  test("a read is retried after a network failure and a 5xx", async () => {
    const { cavea, sent } = world((_, n) =>
      n === 1
        ? network()
        : n === 2
          ? new Response("bad gateway", { status: 502 })
          : json(200, { key: "c" }),
    );
    expect((await cavea.charts.get("c")).key).toBe("c");
    expect(sent).toHaveLength(3);
  });

  test("a call without an idempotency key is never repeated", async () => {
    for (const fail of [
      network,
      () => refused(503, "unavailable"),
      () => refused(429, "rate_limited", {}, { "Retry-After": "1" }),
    ]) {
      const { cavea, sent } = world(fail);
      const error = await rejection(cavea.holdTokens.create({ event: "e" }));
      expect(error).toBeInstanceOf(CaveaError);
      expect(sent).toHaveLength(1);
      const { cavea: c2, sent: s2 } = world(fail);
      await rejection(c2.events.update("e", { name: "x" }));
      await rejection(c2.charts.create({ key: "k", name: "n" }));
      await rejection(c2.charts.saveDraft("c", {}, { ifMatch: '"1"' }));
      expect(s2).toHaveLength(3);
    }
  });

  test("a draft saved over * and an archive are repeated", async () => {
    const { cavea, sent } = world(
      (_, n) => (n % 2 === 1 ? refused(503, "overloaded") : json(200, { key: "c" })),
      {
        maxRetryAfter: 0,
      },
    );
    await cavea.charts.saveDraft("c", {}, { ifMatch: "*" });
    await cavea.events.archive("e");
    expect(sent).toHaveLength(4);
  });

  test("4xx is final; retries stop at maxRetries", async () => {
    const { cavea, sent } = world(() =>
      refused(409, "object_not_free", { objects: [{ label: "A-1", oid: 1, reason: "not_free" }] }),
    );
    const error = (await rejection(
      cavea.events.hold("e", { hold_token: TOKEN, objects: ["A-1"] }),
    )) as CaveaError;
    expect(error.code).toBe("object_not_free");
    expect(sent).toHaveLength(1);
    const { cavea: c2, sent: s2 } = world(() => network(), { maxRetries: 3 });
    const lost = (await rejection(c2.charts.get("c"))) as CaveaError;
    expect(lost.code).toBe("network");
    expect(lost.message).toContain("ECONNRESET");
    expect(s2).toHaveLength(4);
  });

  test("Retry-After is waited for, and one too long is not", async () => {
    const { cavea, sent } = world((_, n) =>
      n === 1 ? refused(429, "rate_limited", {}, { "Retry-After": "1" }) : json(200, SEAT),
    );
    const started = performance.now();
    await cavea.events.book("e", { order_ref: "1" });
    expect(performance.now() - started).toBeGreaterThanOrEqual(990);
    expect(sent).toHaveLength(2);
    const { cavea: c2, sent: s2 } = world(() =>
      refused(503, "overloaded", {}, { "Retry-After": "60" }),
    );
    const error = (await rejection(c2.charts.get("c"))) as CaveaError;
    expect(error.retryAfter).toBe(60);
    expect(s2).toHaveLength(1);
  });
});

describe("timeouts and cancellation", () => {
  test("an attempt that takes too long is a timeout; a read is tried again", async () => {
    let calls = 0;
    const cavea = new Cavea({
      baseUrl: BASE,
      secretKey: KEY,
      timeoutMs: 30,
      maxRetries: 1,
      fetch: (input, init) => {
        calls++;
        return hang({} as Sent, init);
      },
    });
    const error = (await rejection(cavea.charts.get("c"))) as CaveaError;
    expect(error.code).toBe("timeout");
    expect(error.status).toBe(0);
    expect(calls).toBe(2);
  });

  test("the caller's signal rejects with its reason, also while waiting to retry", async () => {
    const reason = new Error("stop");
    const controller = new AbortController();
    const cavea = new Cavea({
      baseUrl: BASE,
      secretKey: KEY,
      fetch: (_, init) => hang({} as Sent, init),
    });
    const pending = rejection(cavea.charts.get("c", { signal: controller.signal }));
    controller.abort(reason);
    expect(await pending).toBe(reason);

    const waiting = new AbortController();
    const { cavea: c2, sent } = world(() => refused(503, "overloaded", {}, { "Retry-After": "2" }));
    const during = rejection(c2.charts.get("c", { signal: waiting.signal }));
    await Bun.sleep(50);
    waiting.abort(reason);
    expect(await during).toBe(reason);
    expect(sent).toHaveLength(1);
  });
});

describe("errors", () => {
  test("a problem's code, reason, details, request id and Retry-After", async () => {
    const { cavea } = world(() =>
      refused(
        400,
        "invalid_request",
        { reason: "invalid_field", field: "objects" },
        { "Retry-After": "3" },
      ),
    );
    const error = (await rejection(
      cavea.events.hold("e", { hold_token: TOKEN, objects: [] }),
    )) as CaveaError;
    expect(error).toBeInstanceOf(CaveaError);
    expect({ ...error, message: error.message }).toEqual({
      name: "CaveaError",
      operation: "hold",
      status: 400,
      code: "invalid_request",
      reason: "invalid_field",
      details: { reason: "invalid_field", field: "objects" },
      requestId: "req_1",
      retryAfter: 3,
      message: "hold: invalid_request, invalid_field, field objects (HTTP 400, req_1)",
    });
  });

  test("a success that is not the operation's JSON is unexpected, not a reply", async () => {
    // A proxy's login page, or nothing: the booking did not reach Cavea.
    for (const answer of [
      () => new Response("<html>sign in</html>", { headers: { "Content-Type": "text/html" } }),
      () => new Response(null, { status: 200 }),
      () => new Response('{"seq":1}', { headers: { "Content-Type": "text/plain" } }),
    ]) {
      const { cavea, sent } = world(answer);
      const error = (await rejection(cavea.events.book("e", { order_ref: "1" }))) as CaveaError;
      expect([error.code, error.status]).toEqual(["unexpected", 200]);
      expect(sent).toHaveLength(1);
    }
    // Binary frames are bytes, as sent; a documented empty answer is nothing.
    const frame = Uint8Array.of(0, 1, 0xff, 0x80, 10);
    const { cavea: c3 } = world((req) =>
      req.url.pathname.endsWith("/state")
        ? new Response(frame, { headers: { "Content-Type": "application/vnd.cavea.frame" } })
        : new Response(null, { status: 204 }),
    );
    const bytes = await c3.call("eventState", { path: { event: "e" } });
    expect(bytes instanceof ArrayBuffer ? [...new Uint8Array(bytes)] : bytes).toEqual([...frame]);
    expect(await c3.charts.unlock("c")).toBeUndefined();
    // Only the documented status and media type: a login page on 200, an
    // empty 200 (as if nothing changed), frames on an undocumented 202.
    for (const answer of [
      () => new Response("<html>sign in</html>", { headers: { "Content-Type": "text/html" } }),
      () => new Response(null, { status: 200 }),
      () =>
        new Response(frame, {
          status: 202,
          headers: { "Content-Type": "application/vnd.cavea.frame" },
        }),
    ]) {
      const { cavea: c4 } = world(answer);
      const error = (await rejection(
        c4.call("eventState", { path: { event: "e" } }),
      )) as CaveaError;
      expect(error.code).toBe("unexpected");
    }
    // A 204 with a body is not the documented empty answer either.
    const { cavea: c5 } = world(() => new Response(new Uint8Array([1]), { status: 204 }));
    expect(((await rejection(c5.charts.unlock("c"))) as CaveaError).code).toBe("unexpected");
    // `metrics` answers text.
    const { cavea } = world(
      () => new Response("cavea_up 1\n", { headers: { "Content-Type": "text/plain" } }),
    );
    expect(await cavea.call("metrics", {})).toBe("cavea_up 1\n");
  });

  test("an answer that is not a problem of the API is unexpected", async () => {
    const { cavea } = world(
      () =>
        new Response("<html>teapot</html>", {
          status: 418,
          headers: { "Content-Type": "text/html" },
        }),
    );
    const error = (await rejection(cavea.charts.get("c"))) as CaveaError;
    expect([error.code, error.status]).toEqual(["unexpected", 418]);
    const { cavea: c2 } = world(
      () => new Response("{", { status: 200, headers: { "Content-Type": "application/json" } }),
    );
    expect(((await rejection(c2.charts.get("c"))) as CaveaError).code).toBe("unexpected");
  });
});

describe("pages", () => {
  test("every item once, empty pages crossed, the cursor passed back exactly", async () => {
    const pages: Record<string, { data: Array<{ label: string }>; next: string | null }> = {
      "": { data: [{ label: "A 1" }], next: "A 1" },
      "A 1": { data: [], next: "Z/9" },
      "Z/9": { data: [{ label: "Z 10" }], next: null },
    };
    const { cavea, sent } = world((req) =>
      json(200, pages[req.url.searchParams.get("after") ?? ""]),
    );
    const labels: string[] = [];
    for await (const object of cavea.events.listObjects("e", { status: "booked", limit: 2 })) {
      labels.push(object.label);
    }
    expect(labels).toEqual(["A 1", "Z 10"]);
    expect(sent.map((s) => s.url.searchParams.get("after"))).toEqual([null, "A 1", "Z/9"]);
    expect(
      sent.every(
        (s) =>
          s.url.searchParams.get("limit") === "2" && s.url.searchParams.get("status") === "booked",
      ),
    ).toBe(true);
  });

  test("a cursor that does not move is an error, not a loop", async () => {
    const { cavea, sent } = world(() => json(200, { data: [{ key: "a" }], next: "a" }));
    const seen: string[] = [];
    const error = await rejection(
      (async () => {
        for await (const chart of cavea.charts.list({ after: "a" })) {
          seen.push(chart.key);
        }
      })(),
    );
    expect(String(error)).toMatch(/same cursor/);
    expect(sent).toHaveLength(1);
  });
});

describe("session tokens", () => {
  test("HS256 over the claims, verifiable with the key's SHA-256 as the server does", async () => {
    const { cavea } = world(() => json(200, {}));
    const before = Math.floor(Date.now() / 1000);
    const token = await cavea.signSessionToken({
      sub: "buyer-1",
      event: "show",
      ttlSeconds: 60,
      maxObjects: 4,
    });
    const [head, body, signature] = token.split(".");
    expect(decode(head)).toEqual({ alg: "HS256", typ: "JWT", kid: KEY_ID });
    const claims = decode(body) as Record<string, number | string>;
    expect(claims["sub"]).toBe("buyer-1");
    expect(claims["event"]).toBe("show");
    expect(claims["max_objects"]).toBe(4);
    expect(Number(claims["exp"]) - before).toBeGreaterThanOrEqual(60);
    expect(Number(claims["exp"]) - before).toBeLessThanOrEqual(61);
    // The server holds SHA-256(key) and keys HMAC with it (D-34).
    const stored = createHash("sha256").update(KEY).digest();
    expect(createHmac("sha256", stored).update(`${head}.${body}`).digest("base64url")).toBe(
      signature ?? "",
    );
    expect(token).not.toContain(KEY);
  });

  test("needs the key id; refuses claims of the wrong shape", async () => {
    const plain = new Cavea({ baseUrl: BASE, secretKey: KEY });
    expect(String(await rejection(plain.signSessionToken({ sub: "a", event: "e" })))).toMatch(
      /keyId/,
    );
    const { cavea } = world(() => json(200, {}));
    for (const claims of [
      { sub: "", event: "e" },
      { sub: "x".repeat(129), event: "e" },
      { sub: "a", event: "e", ttlSeconds: 0 },
      { sub: "a", event: "e", maxObjects: 1.5 },
      // Within each claim's limit, over the server's 1,024 bytes in all.
      { sub: "\u{1F600}".repeat(128), event: "e".repeat(128) },
    ]) {
      expect(await rejection(cavea.signSessionToken(claims))).toBeInstanceOf(TypeError);
    }
  });
});
