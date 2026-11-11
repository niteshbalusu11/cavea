// The SDK end to end against a real `cavea-server` (debug build) on a
// PostgreSQL database of its own, created here and dropped at the end. Runs
// when `CAVEA_TEST_DATABASE_URL` names the server (AGENTS.md "Database"),
// and needs `psql` and `cargo` on PATH (`nix develop`); otherwise skipped.
//
// A workspace and its keys by the admin key, a chart published, events, a
// hold token, hold, book and release; an idempotent replay and a mismatch; a
// conflict; draft etags; page walks; a lost reply retried with its key; a
// `429` waited for; a session token the server accepts.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { join } from "node:path";
import { Cavea, CaveaError, type Fetch } from "../src/index.ts";

const DATABASE = process.env["CAVEA_TEST_DATABASE_URL"];
const ROOT = join(import.meta.dir, "../../..");
const SLOW = 600_000;

/** Two sections of two rows: 4 × 6 seats. */
const DOC = {
  schema: 1,
  name: "Hall",
  nextOid: 1000,
  categories: [{ key: "a", label: "A", color: "#2563EB" }],
  nodes: [
    {
      id: 10,
      type: "section",
      label: "L",
      category: "a",
      children: [
        {
          id: 11,
          type: "rowBlock",
          rows: [
            { rid: 12, seats: 6, oids: [[100, 6]] },
            { rid: 13, seats: 6, oids: [[200, 6]] },
          ],
        },
      ],
    },
    {
      id: 20,
      type: "section",
      label: "R",
      category: "a",
      origin: [900, 0],
      children: [
        {
          id: 21,
          type: "rowBlock",
          rows: [
            { rid: 22, seats: 6, oids: [[300, 6]] },
            { rid: 23, seats: 6, oids: [[400, 6]] },
          ],
        },
      ],
    },
  ],
};
const SEATS = 24;

async function run(cmd: string[], env: Record<string, string> = {}): Promise<string> {
  const proc = Bun.spawn(cmd, {
    cwd: ROOT,
    env: { ...process.env, ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  const [out, err, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (code !== 0) {
    throw new Error(`${cmd[0]} exited with ${code}: ${err.slice(-2000)}`);
  }
  return out;
}

const freePort = (): number => {
  const server = Bun.serve({ port: 0, fetch: () => new Response() });
  const port = server.port ?? 0;
  server.stop(true);
  return port;
};

async function rejection(promise: Promise<unknown>): Promise<CaveaError> {
  try {
    await promise;
  } catch (e) {
    if (e instanceof CaveaError) {
      return e;
    }
    throw e;
  }
  throw new Error("expected a CaveaError");
}

async function all<T>(items: AsyncIterable<T>): Promise<T[]> {
  const out: T[] = [];
  for await (const item of items) {
    out.push(item);
  }
  return out;
}

describe.skipIf(DATABASE === undefined)("against cavea-server", () => {
  const admin = randomBytes(24).toString("hex");
  const db = `cavea_sdk_e2e_${process.pid}_${randomBytes(4).toString("hex")}`;
  const server = (DATABASE ?? "").replace(/\/[^/]*$/, "");
  let base = "";
  let proc: Bun.Subprocess | undefined;
  let log = "";

  beforeAll(async () => {
    await run(["psql", DATABASE ?? "", "-qc", `CREATE DATABASE ${db}`]);
    await run(["cargo", "build", "-q", "-p", "cavea-server"], { CARGO_BUILD_JOBS: "8" });
    const port = freePort();
    base = `http://127.0.0.1:${port}`;
    proc = Bun.spawn([join(ROOT, "target/debug/cavea-server")], {
      cwd: ROOT,
      env: {
        ...process.env,
        CAVEA_DATABASE_URL: `${server}/${db}`,
        CAVEA_LISTEN: `127.0.0.1:${port}`,
        CAVEA_ADMIN_KEY: admin,
        CAVEA_LOG_FORMAT: "text",
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    for (const stream of [proc.stdout, proc.stderr]) {
      if (stream instanceof ReadableStream) {
        void (async () => {
          const decoder = new TextDecoder();
          for await (const chunk of stream) {
            log = (log + decoder.decode(chunk)).slice(-200_000);
          }
        })();
      }
    }
    for (let i = 0; ; i++) {
      const ok = await fetch(`${base}/readyz`).then(
        (r) => r.ok,
        () => false,
      );
      if (ok) {
        break;
      }
      if (i > 150 || proc.exitCode !== null) {
        throw new Error(`cavea-server did not become ready:\n${log.slice(-2000)}`);
      }
      await Bun.sleep(200);
    }
  }, SLOW);

  afterAll(async () => {
    if (proc !== undefined) {
      proc.kill();
      await proc.exited;
    }
    await run(["psql", DATABASE ?? "", "-qc", `DROP DATABASE IF EXISTS ${db} WITH (FORCE)`]);
  }, SLOW);

  test(
    "a backend's whole flow",
    async () => {
      // The operator: a workspace and its first secret key.
      const operator = new Cavea({ baseUrl: base, adminKey: admin });
      const ws = await operator.workspaces.create({ key: "sdk-e2e", name: "SDK" });
      const listed = await all(operator.workspaces.list({ limit: 1 }));
      expect(listed.map((w) => w.id)).toContain(ws.id);
      const cavea = new Cavea({
        baseUrl: base,
        secretKey: ws.secret_key.key,
        keyId: ws.secret_key.id,
      });

      // A chart, published; publishing again finds the same version.
      await cavea.charts.create({ key: "hall", name: "Hall", document: DOC });
      expect((await cavea.charts.validate("hall")).valid).toBe(true);
      const first = await cavea.charts.publish("hall");
      expect([first.created, first.version.number, first.version.stats.seats]).toEqual([
        true,
        1,
        SEATS,
      ]);
      const again = await cavea.charts.publish("hall");
      expect([again.created, again.version.id]).toEqual([false, first.version.id]);

      // Draft etags: a save with the etag read, then a stale one refused.
      const draft = await cavea.charts.getDraft("hall");
      const saved = await cavea.charts.saveDraft(
        "hall",
        { ...draft.document, name: "Hall 2" },
        { ifMatch: draft.etag },
      );
      const stale = await rejection(
        cavea.charts.saveDraft("hall", draft.document, { ifMatch: draft.etag }),
      );
      expect([stale.status, stale.code, stale.details["draft_etag"]]).toEqual([
        409,
        "version_conflict",
        saved.draft_etag,
      ]);
      expect(stale.requestId).toMatch(/^req_[0-9a-f]{32}$/);
      expect((await all(cavea.charts.versions("hall"))).map((v) => v.number)).toEqual([1]);

      // Events, walked two to a page.
      for (const key of ["show", "e1", "e2", "e3", "e4"]) {
        await cavea.events.create({ key, chart: "hall" });
      }
      const pages = await all(cavea.events.list({ limit: 2 }).pages());
      expect(pages.map((p) => p.data.length)).toEqual([2, 2, 1]);
      expect(pages.flatMap((p) => p.data.map((e) => e.key))).toEqual([
        "show",
        "e1",
        "e2",
        "e3",
        "e4",
      ]);
      expect((await all(cavea.events.list({ chart: "hall" }))).length).toBe(5);

      // Every object once, seven to a page.
      const objects = await all(cavea.events.listObjects("show", { limit: 7 }));
      const labels = objects.map((o) => o.label);
      expect(labels.length).toBe(SEATS);
      expect(new Set(labels).size).toBe(SEATS);
      const [a = "", b = "", c = "", d = ""] = labels;

      // Hold under a token, book with an order number as the key.
      const token = await cavea.holdTokens.create({ event: "show" });
      const held = await cavea.events.hold("show", {
        hold_token: token.hold_token,
        objects: [a, b],
      });
      expect(held.objects.map((o) => o.label)).toEqual([a, b]);
      expect((await cavea.holdTokens.get(token.hold_token)).objects.length).toBe(2);
      const extended = await cavea.holdTokens.extend(token.hold_token, { ttl_seconds: 600 });
      expect(extended.seq).toBeGreaterThan(held.seq);
      const order = { hold_token: token.hold_token, order_ref: "77120" };
      const booked = await cavea.events.book("show", order, { idempotencyKey: "order-77120" });
      // The same request again replays the reply (the token is gone by now).
      const replay = await cavea.events.book("show", order, { idempotencyKey: "order-77120" });
      expect(replay).toEqual(booked);
      const mismatch = await rejection(
        cavea.events.book(
          "show",
          { ...order, order_ref: "77121" },
          { idempotencyKey: "order-77120" },
        ),
      );
      expect([mismatch.status, mismatch.code]).toEqual([409, "idempotency_mismatch"]);

      // A conflict names the object.
      const other = await cavea.holdTokens.create({ event: "show" });
      const taken = await rejection(
        cavea.events.hold("show", { hold_token: other.hold_token, objects: [a, c] }),
      );
      expect(taken.code).toBe("object_not_free");
      expect(taken.details["objects"]).toEqual([
        { label: a, oid: expect.any(Number), reason: "not_free" },
      ]);

      // Status pages and objects by label.
      expect(
        (await all(cavea.events.listObjects("show", { status: "booked", limit: 1 }))).map(
          (o) => o.label,
        ),
      ).toEqual([a, b]);
      await cavea.events.release("show", { objects: [b] });
      const named = await cavea.events.objects("show", [a, b, "nope"]);
      expect(named.data.map((o) => [o.label, o.status, o.order_ref ?? null])).toEqual([
        [a, "booked", "77120"],
        [b, "free", null],
      ]);
      expect(named.unknown).toEqual(["nope"]);

      // Block and unblock; release a token's objects.
      await cavea.events.changeStatus("show", { objects: [d], status: "blocked" });
      await cavea.events.changeStatus("show", { objects: [d], status: "free" });
      await cavea.events.hold("show", { hold_token: other.hold_token, objects: [c] });
      expect(
        (await cavea.holdTokens.release(other.hold_token)).objects.map((o) => o.label),
      ).toEqual([c]);

      // Events change and archive.
      expect((await cavea.events.update("e4", { name: "Matinee" })).name).toBe("Matinee");
      expect((await cavea.events.archive("e4")).archived_at).not.toBeNull();
      expect((await cavea.events.get("show")).counts?.booked).toBe(1);

      // Keys.
      const pk = await cavea.keys.create({ kind: "publishable", name: "web" });
      expect((await all(cavea.keys.list())).map((k) => k.id)).toEqual([ws.secret_key.id, pk.id]);
      const self = await rejection(cavea.keys.revoke(ws.secret_key.id));
      expect([self.status, self.reason]).toEqual([403, "self_revocation"]);

      // A session token signed here is accepted by the server's public route.
      await cavea.events.create({
        key: "gated",
        chart: "hall",
        settings: { require_session: true },
      });
      const publicToken = (session?: string): Promise<Response> =>
        fetch(`${base}/v1/public/hold-tokens`, {
          method: "POST",
          headers: {
            Authorization: `Bearer ${pk.key}`,
            "Content-Type": "application/json",
            ...(session === undefined ? {} : { "Cavea-Session": session }),
          },
          body: JSON.stringify({ event: "gated" }),
        });
      const session = await cavea.signSessionToken({
        sub: "buyer-1",
        event: "gated",
        maxObjects: 3,
      });
      const accepted = await publicToken(session);
      expect(accepted.status).toBe(201);
      expect(((await accepted.json()) as { max_objects: number }).max_objects).toBe(3);
      expect((await publicToken()).status).toBe(401);
      expect((await publicToken(`${session.slice(0, -2)}AA`)).status).toBe(401);
      const wrongEvent = await cavea.signSessionToken({ sub: "buyer-1", event: "show" });
      expect(
        ((await (await publicToken(wrongEvent)).json()) as { details: unknown }).details,
      ).toEqual({
        reason: "session_token",
        session: "event",
      });

      // Nothing the client sent shows up in the server's log.
      expect(log).not.toContain(ws.secret_key.key);
      expect(log).not.toContain(token.hold_token);
    },
    SLOW,
  );

  test(
    "a lost reply is retried with its key and applied once; a 429 is waited for",
    async () => {
      const operator = new Cavea({ baseUrl: base, adminKey: admin });
      const ws = await operator.workspaces.create({ key: "sdk-faults", name: "Faults" });
      const plain = new Cavea({ baseUrl: base, secretKey: ws.secret_key.key });
      await plain.charts.create({ key: "hall", name: "Hall", document: DOC });
      await plain.charts.publish("hall");
      await plain.events.create({ key: "show", chart: "hall" });
      const [a = "", b = ""] =
        (await all(plain.events.listObjects("show", { limit: 2 }).pages()))[0]?.data.map(
          (o) => o.label,
        ) ?? [];

      // The first hold reaches the server, but its reply is lost on the way back.
      const seen: Array<{ op: string; key: string | null; status: number | "lost" | "synthetic" }> =
        [];
      let loseNextHold = true;
      let refuseNextBook = true;
      const faulty: Fetch = async (input, init) => {
        const path = new URL(input).pathname;
        const key = new Headers(init.headers).get("Idempotency-Key");
        if (path.endsWith("/book") && refuseNextBook) {
          refuseNextBook = false;
          seen.push({ op: "book", key, status: "synthetic" });
          return new Response(
            JSON.stringify({
              code: "rate_limited",
              status: 429,
              message: "slow down",
              details: {},
              request_id: "req_x",
            }),
            {
              status: 429,
              headers: { "Content-Type": "application/problem+json", "Retry-After": "1" },
            },
          );
        }
        const response = await fetch(input, init);
        if (path.endsWith("/hold") && loseNextHold) {
          loseNextHold = false;
          await response.text();
          seen.push({ op: "hold", key, status: "lost" });
          throw new TypeError("fetch failed", { cause: { code: "ECONNRESET" } });
        }
        seen.push({ op: path.split("/").at(-1) ?? "", key, status: response.status });
        return response;
      };
      const cavea = new Cavea({ baseUrl: base, secretKey: ws.secret_key.key, fetch: faulty });
      const token = await cavea.holdTokens.create({ event: "show" });
      const held = await cavea.events.hold("show", {
        hold_token: token.hold_token,
        objects: [a, b],
      });
      const holds = seen.filter((s) => s.op === "hold");
      expect(holds.map((s) => s.status)).toEqual(["lost", 200]);
      expect(holds[0]?.key).toBe(holds[1]?.key ?? "");
      // Applied once: one hold command in the log (its seq), the two objects held.
      expect((await cavea.holdTokens.get(token.hold_token)).objects.map((o) => o.label)).toEqual([
        a,
        b,
      ]);
      expect((await cavea.events.get("show")).counts?.held).toBe(2);
      const extended = await cavea.holdTokens.extend(token.hold_token);
      expect(extended.seq).toBe(held.seq + 1);

      const started = performance.now();
      const booked = await cavea.events.book("show", {
        hold_token: token.hold_token,
        order_ref: "1",
      });
      expect(performance.now() - started).toBeGreaterThanOrEqual(990);
      const books = seen.filter((s) => s.op === "book");
      expect(books.map((s) => s.status)).toEqual(["synthetic", 200]);
      expect(books[0]?.key).toBe(books[1]?.key ?? "");
      expect(booked.objects.length).toBe(2);
    },
    SLOW,
  );
});
