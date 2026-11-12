// The upgrade test (docs/13-testing-benchmarks.md §13, D-85), run by
// `cargo xtask upgrade`, which builds the binaries and the old renderer:
//
//   bun tools/upgrade/run.ts --label <rev> --new <dir> --old <dir> [--licence <file>] --out <dir>
//   bun tools/upgrade/run.ts --label <rev> --new <dir> --renderer <dist> --out <dir>
//
// `<dir>` holds `cavea-server` and `cavea` (the CLI). The first form fills a
// database of its own with the old server, through the API that revision
// has (its own OpenAPI document says which operations exist): workspaces,
// keys, people, charts with versions, thumbnails, a draft mid-edit and a
// lock, an uploaded image, events with holds live at the stop, bookings,
// blocked seats, a licence; reads every object back; stops the old server;
// starts the new one on the same database; reads everything again and
// compares; then works the data (the old holds, the draft, the lock, the old
// session cookie, signing in, publishing) and runs the reconciler. The
// second form runs the given `@cavea/renderer` package in headless Chromium
// against the new server: it must hold a seat and see a seat change made by
// the backend. Every database is dropped and every process stopped at the
// end. Exit 0 passed, 1 a check failed or old data answers differently, 3
// the test could not run.
import { SQL } from "bun";
import { createHash, randomBytes } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Browser, pinned } from "../chromium.ts";
import { freePort, Server } from "../e2e/server.ts";

const ROOT = join(import.meta.dir, "../..");
const args = process.argv.slice(2);
const flag = (name: string): string | undefined => {
  const i = args.indexOf(name);
  return i === -1 ? undefined : args[i + 1];
};
const label = flag("--label") ?? "?";
const newDir = flag("--new");
const oldDir = flag("--old");
const rendererDir = flag("--renderer");
const licenceFile = flag("--licence");
const OUT = flag("--out") ?? join(ROOT, "target/upgrade/out");
if (newDir === undefined || (oldDir === undefined) === (rendererDir === undefined)) {
  console.error(
    "usage: bun tools/upgrade/run.ts --label <rev> --new <dir> (--old <dir> [--licence <file>] | --renderer <dist>) --out <dir>",
  );
  process.exit(2);
}
const PG = (
  process.env["CAVEA_TEST_DATABASE_URL"] ?? "postgres://cavea:cavea@127.0.0.1:5432/cavea"
).replace(/\/[^/]*$/, "");
const hex = (n: number): string => randomBytes(n).toString("hex");
const started = performance.now();
const say = (text: string): void =>
  console.log(`[${((performance.now() - started) / 1000).toFixed(1)} s] ${text}`);

const cleanup: Array<() => Promise<void>> = [];
const findings: string[] = [];
const notes: string[] = [];
function check(ok: unknown, what: string): void {
  if (!ok) throw new Error(`failed: ${what}`);
}

// ------------------------------------------------------------------ HTTP

type Json = unknown;
interface Reply {
  status: number;
  json: Json;
  bytes: Uint8Array;
  headers: Headers;
}
interface Session {
  cookie: string;
  csrf: string;
  workspace: string;
}
interface As {
  key?: string;
  session?: Session;
  headers?: Record<string, string>;
  raw?: Uint8Array;
  type?: string;
}

let base = "";
async function call(method: string, path: string, body?: Json, as: As = {}): Promise<Reply> {
  const headers: Record<string, string> = { origin: base, ...as.headers };
  if (as.key) headers["authorization"] = `Bearer ${as.key}`;
  if (as.session) {
    headers["cookie"] = as.session.cookie;
    headers["cavea-csrf-token"] = as.session.csrf;
    if (as.session.workspace) headers["cavea-workspace"] = as.session.workspace;
  }
  let payload: BodyInit | undefined;
  if (as.raw) {
    headers["content-type"] = as.type ?? "application/octet-stream";
    payload = as.raw;
  } else if (body !== undefined) {
    headers["content-type"] = "application/json";
    payload = JSON.stringify(body);
  }
  const response = await fetch(base + path, { method, headers, body: payload });
  const bytes = new Uint8Array(await response.arrayBuffer());
  let json: Json = null;
  if ((response.headers.get("content-type") ?? "").includes("json") && bytes.length > 0) {
    json = JSON.parse(new TextDecoder().decode(bytes));
  }
  return { status: response.status, json, bytes, headers: response.headers };
}

/** `call`, failing unless the status is one of `ok`. */
async function must(
  ok: number[],
  method: string,
  path: string,
  body?: Json,
  as: As = {},
): Promise<Reply> {
  const reply = await call(method, path, body, as);
  if (!ok.includes(reply.status)) {
    throw new Error(
      `${method} ${path}: ${reply.status} ${JSON.stringify(reply.json ?? "").slice(0, 400)}`,
    );
  }
  return reply;
}

function at(value: Json, ...path: (string | number)[]): Json {
  return path.reduce<Json>((v, k) => (v as Record<string | number, Json> | null)?.[k], value);
}
const str = (value: Json, ...path: (string | number)[]): string => String(at(value, ...path));

// ------------------------------------------------------------- processes

async function database(): Promise<string> {
  const name = `cavea_upgrade_${process.pid}_${hex(3)}`;
  const admin = new SQL(`${PG}/cavea`, { max: 1 });
  await admin.unsafe(`CREATE DATABASE ${name}`);
  cleanup.push(async () => {
    await admin.unsafe(`DROP DATABASE IF EXISTS ${name} WITH (FORCE)`);
    await admin.close();
  });
  return `${PG}/${name}`;
}

function cli(dir: string, ...argv: string[]): string {
  const run = Bun.spawnSync([join(dir, "cavea"), ...argv], { stderr: "pipe" });
  if (run.exitCode !== 0) throw new Error(`cavea ${argv.join(" ")}: ${run.stderr.toString()}`);
  return run.stdout.toString();
}

async function server(dir: string, env: Record<string, string>, name: string): Promise<Server> {
  const out = join(OUT, name);
  await mkdir(join(out, "tmp"), { recursive: true });
  const s = new Server(join(dir, "cavea-server"), { ...env, TMPDIR: join(out, "tmp") }, base, out);
  cleanup.push(async () => void (await s.stop()));
  await s.start();
  return s;
}

// --------------------------------------------------------------- upgrade

/** A 1×1 PNG, the uploaded image. */
const PNG = Uint8Array.from(
  atob(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  ),
  (c) => c.charCodeAt(0),
);

interface World {
  ops: Set<string>;
  admin: string;
  sk: string;
  pk: string;
  owner?: { email: string; password: string; session: Session };
  /** Where the draft and the lock live: the owner's workspace, or `upgrade`'s key. */
  editor: As;
  holds: { staff: string; buyer: string };
  labels: Record<string, string>;
  asset?: string;
  draftEtag?: string;
  /** The optional reads the old server answered. */
  optional: Set<string>;
}

async function populate(oldServer: Server, ops: Set<string>, admin: string): Promise<World> {
  const has = (op: string): boolean => ops.has(op);
  let owner: World["owner"];
  if (has("consoleSetup")) {
    const link = oldServer.log.match(/setup_link"?[:=]"?([^"\s]+)/)?.[1];
    check(link?.includes("#token="), "the old server logged a setup link");
    const email = "owner@example.org";
    const password = hex(12);
    const reply = await must([201], "POST", "/console/api/setup", {
      token: link!.split("#token=")[1],
      email,
      password,
    });
    const cookie = (reply.headers.get("set-cookie") ?? "").split(";")[0] ?? "";
    owner = {
      email,
      password,
      session: {
        cookie,
        csrf: str(reply.json, "csrf_token"),
        workspace: str(reply.json, "workspaces", 0, "id"),
      },
    };
    say("owner set up from the old server's setup link");
  }
  // Only an owner of every workspace may upload a licence: before the admin
  // makes more.
  if (owner && has("consoleLicence") && licenceFile !== undefined) {
    const licence = await Bun.file(licenceFile).text();
    await must(
      [200],
      "PUT",
      "/console/api/licence",
      { licence },
      { session: { ...owner.session, workspace: "" } },
    );
    say("a licence uploaded");
  }
  const ids: string[] = [];
  for (const key of ["upgrade", "neighbour"]) {
    ids.push(
      str(
        (
          await must(
            [201],
            "POST",
            "/v1/workspaces",
            { key, name: `Upgrade ${key}` },
            { key: admin },
          )
        ).json,
        "id",
      ),
    );
  }
  const sk = str(
    (
      await must(
        [201],
        "POST",
        `/v1/workspaces/${ids[0]}/keys`,
        { name: "backend" },
        { key: admin },
      )
    ).json,
    "key",
  );
  const sk2 = str(
    (
      await must(
        [201],
        "POST",
        `/v1/workspaces/${ids[1]}/keys`,
        { name: "backend" },
        { key: admin },
      )
    ).json,
    "key",
  );
  if (has("inviteWorkspaceOwner")) {
    await must(
      [201],
      "POST",
      `/v1/workspaces/${ids[0]}/invitations`,
      { email: "second@example.org" },
      { key: admin },
    );
  }
  const as = { key: sk };
  const pk = str(
    (
      await must(
        [201],
        "POST",
        "/v1/keys",
        { kind: "publishable", name: "buyers", allowed_origins: null },
        as,
      )
    ).json,
    "key",
  );
  const spare = await must([201], "POST", "/v1/keys", { kind: "secret", name: "retired" }, as);
  await must([200], "DELETE", `/v1/keys/${str(spare.json, "id")}`, undefined, as);
  say("workspaces, keys (one revoked)");

  const doc = JSON.parse(cli(oldDir!, "gen", "theatre"));
  await must([201], "POST", "/v1/charts", { key: "hall", name: "Hall", document: doc }, as);
  await must([200, 201], "POST", "/v1/charts/hall/publish", undefined, as);
  const renamed = { ...doc, name: `${String(doc.name ?? "Hall")} (2)` };
  const draft = await must([200], "GET", "/v1/charts/hall/draft", undefined, as);
  await must([200], "PUT", "/v1/charts/hall/draft", renamed, {
    ...as,
    headers: { "if-match": draft.headers.get("etag") ?? "*" },
  });
  await must([200, 201], "POST", "/v1/charts/hall/publish", undefined, as);
  let asset: string | undefined;
  if (has("uploadAsset")) {
    asset = str(
      (
        await must([200, 201], "POST", "/v1/assets", undefined, {
          ...as,
          raw: PNG,
          type: "image/png",
        })
      ).json,
      "hash",
    );
    if (has("setVersionThumbnail")) {
      await must([200, 204], "PUT", "/v1/charts/hall/versions/1/thumbnail", { asset }, as);
    }
  }
  await must([201], "POST", "/v1/charts", { key: "gone", name: "Gone", document: doc }, as);
  await must([200], "DELETE", "/v1/charts/gone", undefined, as);
  await must(
    [201],
    "POST",
    "/v1/charts",
    { key: "hall", name: "Hall", document: doc },
    { key: sk2 },
  );
  say(`charts: two versions${asset ? ", a thumbnail" : ""}, one archived; a neighbour's`);

  // The draft mid-edit and the lock: a person's when the console exists.
  const editor: As = owner ? { session: owner.session } : as;
  await must([201], "POST", "/v1/charts", { key: "draft", name: "Draft", document: doc }, editor);
  const before = await must([200], "GET", "/v1/charts/draft/draft", undefined, editor);
  const edited = { ...doc, name: "Draft, being edited" };
  const saved = await must([200], "PUT", "/v1/charts/draft/draft", edited, {
    ...editor,
    headers: { "if-match": before.headers.get("etag") ?? "*" },
  });
  const draftEtag = saved.headers.get("etag") ?? undefined;
  if (has("lockChart")) {
    await must([200], "POST", "/v1/charts/draft/lock", {}, editor);
  }
  say(`a draft mid-edit${has("lockChart") ? ", locked by its editor" : ""}`);

  // Events and seats.
  await must([201], "POST", "/v1/events", { key: "night", chart: "hall", name: "Night" }, as);
  await must([201], "POST", "/v1/events", { key: "matinee", chart: "hall" }, as);
  await must([200], "DELETE", "/v1/events/matinee", undefined, as);
  const objects = await must(
    [200],
    "GET",
    "/v1/events/night/objects?status=free&limit=12",
    undefined,
    as,
  );
  const free = (at(objects.json, "data") as Json[]).map((o) => str(o, "label"));
  check(free.length === 12, "twelve free seats");
  const labels = {
    held: free[0]!,
    booked: free[1]!,
    bookedToo: free[2]!,
    direct: free[3]!,
    blocked: free[4]!,
    blockedToo: free[5]!,
    released: free[6]!,
    buyer: free[7]!,
    buyerToo: free[8]!,
    later: free[9]!,
    laterToo: free[10]!,
    spare: free[11]!,
  };
  const staff = str(
    (await must([201], "POST", "/v1/hold-tokens", { event: "night", ttl_seconds: 3600 }, as)).json,
    "hold_token",
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/hold",
    { hold_token: staff, objects: [labels.held, labels.booked, labels.bookedToo, labels.released] },
    as,
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/book",
    { hold_token: staff, objects: [labels.booked, labels.bookedToo], order_ref: "order-1" },
    as,
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/release",
    { hold_token: staff, objects: [labels.released] },
    as,
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/book",
    { objects: [labels.direct], order_ref: "order-2" },
    as,
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/change-status",
    { objects: [labels.blocked, labels.blockedToo], status: "blocked" },
    as,
  );
  const buyer = str(
    (await must([201], "POST", "/v1/public/hold-tokens", { event: "night" }, { key: pk })).json,
    "hold_token",
  );
  await must(
    [200],
    "POST",
    "/v1/public/events/night/hold",
    { hold_token: buyer, objects: [labels.buyer, labels.buyerToo] },
    { key: pk },
  );
  say(
    "an event: seats held (staff and buyer, live at the stop), booked, released, blocked; an archived event",
  );

  if (owner && has("inviteMember")) {
    await must(
      [201],
      "POST",
      "/console/api/members",
      { email: "viewer@example.org", role: "viewer" },
      { session: owner.session },
    );
  }
  return {
    ops,
    admin,
    sk,
    pk,
    owner,
    editor,
    holds: { staff, buyer },
    labels,
    asset,
    draftEtag,
    optional: new Set(),
  };
}

/** Every object the old server made, as each read route answers, by route. */
async function snapshot(w: World, old: boolean): Promise<Map<string, Json>> {
  const has = (op: string): boolean => w.ops.has(op);
  const shot = new Map<string, Json>();
  /** Reads `path` into the snapshot; an `optional` one only where the old server had it. */
  const get = async (path: string, as: As, optional = false): Promise<Json> => {
    const reply = await call("GET", path, undefined, as);
    if (optional && (old ? reply.status !== 200 : !w.optional.has(path))) {
      return null;
    }
    if (optional && old) w.optional.add(path);
    // Every read succeeds on the old server; on the new one a failure is a difference.
    check(!old || reply.status === 200, `the old server answers GET ${path} (${reply.status})`);
    const value = reply.json ?? {
      bytes: reply.bytes.length,
      sha256: createHash("sha256").update(reply.bytes).digest("hex"),
    };
    shot.set(
      `GET ${path} (${as.session ? "session" : as.key === w.admin ? "admin" : as.key === w.pk ? "publishable" : "secret"})`,
      { status: reply.status, body: value, etag: reply.headers.get("etag") },
    );
    return value;
  };
  const as = { key: w.sk };
  await get("/v1/workspaces", { key: w.admin });
  await get("/v1/keys", as);
  for (const chart of ["hall", "gone"]) {
    await get(`/v1/charts/${chart}`, as);
    await get(`/v1/charts/${chart}/versions`, as);
    await get(`/v1/charts/${chart}/draft`, as);
  }
  await get("/v1/charts", as);
  await get("/v1/charts/draft", w.editor);
  await get("/v1/charts/draft/draft", w.editor);
  if (has("getChartLock")) await get("/v1/charts/draft/lock", w.editor);
  await get("/v1/events", as);
  // Archived events are listed on request only since #82.
  await get("/v1/events?archived=true", as, true);
  const night = await get("/v1/events/night", as);
  await get("/v1/events/matinee", as);
  let after = "";
  for (let page = 0; page < 100; page++) {
    const objects = await get(`/v1/events/night/objects?limit=100${after}`, as);
    const next = at(objects, "next");
    if (typeof next !== "string" || next === "") break;
    after = `&after=${encodeURIComponent(next)}`;
  }
  await get(`/v1/hold-tokens/${w.holds.staff}`, as);
  await get("/v1/public/events/night", { key: w.pk });
  const url = at(night, "version", "url");
  if (typeof url === "string") await get(url, {});
  if (w.asset !== undefined) await get(`/v1/public/assets/${w.asset}`, {});
  if (w.owner) {
    const session = { session: w.owner.session };
    await get("/console/api/me", session);
    if (has("listMembers")) await get("/console/api/members", session);
    if (has("listInvitations")) await get("/console/api/invitations", session);
    if (has("listAudit")) await get("/console/api/audit?limit=100", session);
  }
  return shot;
}

/** Fields that change by reading: a key's last use; an error's request id. */
const VOLATILE = new Set(["last_used_at", "request_id"]);

/** The differences between what the old server answered and what the new one does. */
function compare(
  before: Map<string, Json>,
  after: Map<string, Json>,
): { changed: string[]; added: string[] } {
  const changed: string[] = [];
  const added = new Set<string>();
  const walk = (route: string, path: string, a: Json, b: Json): void => {
    if (a !== null && b !== null && typeof a === "object" && typeof b === "object") {
      if (Array.isArray(a) !== Array.isArray(b)) {
        changed.push(`${route} ${path}: an array became an object or the reverse`);
        return;
      }
      const ka = Object.keys(a as object);
      const kb = new Set(Object.keys(b as object));
      if (Array.isArray(a) && ka.length !== kb.size) {
        changed.push(`${route} ${path}: ${ka.length} items before, ${kb.size} after`);
      }
      for (const k of ka) {
        if (VOLATILE.has(k)) continue;
        if (!kb.has(k)) {
          changed.push(`${route} ${path}.${k}: gone`);
          continue;
        }
        walk(route, `${path}.${k}`, (a as Record<string, Json>)[k], (b as Record<string, Json>)[k]);
      }
      for (const k of kb) {
        if (!Object.hasOwn(a as object, k)) {
          added.add(`${path.replace(/\.\d+/g, "[]")}.${k}`);
        }
      }
      return;
    }
    if (JSON.stringify(a) !== JSON.stringify(b)) {
      changed.push(
        `${route} ${path}: ${JSON.stringify(a)?.slice(0, 120)} -> ${JSON.stringify(b)?.slice(0, 120)}`,
      );
    }
  };
  for (const [route, a] of before) {
    walk(route, "", a, after.get(route) ?? null);
  }
  return { changed, added: [...added] };
}

async function upgrade(): Promise<void> {
  const url = await database();
  const port = freePort();
  base = `http://localhost:${port}`;
  const admin = hex(24);
  const env = {
    CAVEA_DATABASE_URL: url,
    CAVEA_LISTEN: `127.0.0.1:${port}`,
    CAVEA_ADMIN_KEY: admin,
    CAVEA_PUBLIC_URL: base,
  };
  const sql = new SQL(url, { max: 1 });
  cleanup.push(() => sql.close());
  const migrations = async (): Promise<string> => {
    const rows = (await sql.unsafe(
      "SELECT version FROM cavea_migrations ORDER BY version",
    )) as Array<{ version: bigint }>;
    return rows.map((r) => String(r.version)).join(",");
  };

  const old = await server(oldDir!, env, "old");
  const document = (await must([200], "GET", "/v1/openapi.json")).json;
  const ops = new Set<string>();
  for (const methods of Object.values((at(document, "paths") ?? {}) as Record<string, Json>)) {
    for (const op of Object.values(methods as Record<string, Json>))
      ops.add(str(op, "operationId"));
  }
  say(`old server (${label}) up: ${ops.size} operations, migrations ${await migrations()}`);
  const world = await populate(old, ops, admin);
  const before = await snapshot(world, true);
  say(`read back ${before.size} routes from the old server`);

  const stop = await old.stop();
  check(stop.code === 0, `the old server stopped cleanly (exit ${stop.code})`);
  say(`old server stopped in ${stop.seconds.toFixed(1)} s with two hold tokens live`);
  const applied = await migrations();

  const next = await server(newDir!, env, "new");
  const now = await migrations();
  say(`new server up on the same database: migrations ${applied} -> ${now}`);
  const after = await snapshot(world, false);
  const { changed, added } = compare(before, after);
  for (const c of changed) findings.push(`old data answers differently: ${c}`);
  if (added.length > 0)
    notes.push(`fields the new server adds to old objects: ${added.toSorted().join(", ")}`);
  say(
    `compared ${before.size} routes: ${changed.length} difference(s), ${added.length} added field(s)`,
  );

  const verify = (): string => {
    const run = Bun.spawnSync([join(newDir!, "cavea-server"), "verify", "--all"], {
      env: { ...process.env, ...env },
      stderr: "pipe",
    });
    if (run.exitCode !== 0) {
      findings.push(
        `the reconciler is not clean (exit ${run.exitCode}): ${run.stdout.toString().slice(0, 600)}${run.stderr.toString().slice(0, 300)}`,
      );
    }
    return run.stdout.toString().trim().split("\n").at(-1) ?? "";
  };
  say(`reconciler on the upgraded database: ${verify()}`);

  // The data still works.
  const as = { key: world.sk };
  const l = world.labels;
  const token = await call("GET", `/v1/hold-tokens/${world.holds.staff}`, undefined, as);
  check(token.status === 200, `the old staff hold token answers (${token.status})`);
  await must(
    [200],
    "POST",
    `/v1/hold-tokens/${world.holds.staff}/extend`,
    { ttl_seconds: 600 },
    as,
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/book",
    { hold_token: world.holds.staff, objects: [l.held], order_ref: "order-3" },
    as,
  );
  await must(
    [200],
    "POST",
    "/v1/public/events/night/release",
    { hold_token: world.holds.buyer, objects: [l.buyerToo] },
    { key: world.pk },
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/book",
    { hold_token: world.holds.buyer, objects: [l.buyer], order_ref: "order-4" },
    as,
  );
  const fresh = str(
    (await must([201], "POST", "/v1/public/hold-tokens", { event: "night" }, { key: world.pk }))
      .json,
    "hold_token",
  );
  await must(
    [200],
    "POST",
    "/v1/public/events/night/hold",
    { hold_token: fresh, objects: [l.later] },
    { key: world.pk },
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/book",
    { hold_token: fresh, order_ref: "order-5" },
    as,
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/change-status",
    { objects: [l.blocked], status: "free" },
    as,
  );
  const counts = at((await must([200], "GET", "/v1/events/night", undefined, as)).json, "counts");
  check(
    at(counts, "booked") === 6,
    `six seats booked after the upgrade: ${JSON.stringify(counts)}`,
  );
  say(
    "old holds extended and booked, the buyer's released and booked, a new hold, a seat unblocked",
  );

  const saved = await call(
    "PUT",
    "/v1/charts/draft/draft",
    { ...JSON.parse(cli(oldDir!, "gen", "theatre")), name: "Draft, edited after the upgrade" },
    {
      ...world.editor,
      headers: { "if-match": world.draftEtag ?? "*" },
    },
  );
  if (saved.status !== 200)
    findings.push(
      `the draft's edit could not continue with the old ETag: ${saved.status} ${JSON.stringify(saved.json)}`,
    );
  if (ops.has("getChartLock")) {
    const lock = await must([200], "GET", "/v1/charts/draft/lock", undefined, world.editor);
    notes.push(
      `the editor's lock after the upgrade: ${at(lock.json, "lock") === null ? "gone (expired during the test)" : `still held by ${str(lock.json, "lock", "holder")}`}`,
    );
    await must([200, 204], "DELETE", "/v1/charts/draft/lock", undefined, world.editor);
  }
  await must([200, 201], "POST", "/v1/charts/hall/publish", undefined, as);
  await must([201], "POST", "/v1/events", { key: "after", chart: "hall" }, as);
  say(
    `the draft edited on with its old ETag${ops.has("getChartLock") ? "; the lock released" : ""}; a chart published and an event made`,
  );

  if (world.owner) {
    const me = await call("GET", "/console/api/me", undefined, { session: world.owner.session });
    notes.push(
      `a session cookie the old server issued: ${me.status === 200 ? "still signed in (sessions live in the database)" : `answered ${me.status}`}`,
    );
    if (me.status !== 200)
      findings.push(
        `the old server's session cookie answers ${me.status}: an upgrade signs everyone out`,
      );
    const signIn = await must([200], "POST", "/console/api/session", {
      email: world.owner.email,
      password: world.owner.password,
    });
    check(
      str(signIn.json, "user", "email") === world.owner.email,
      "the owner signs in on the new server",
    );
    if (licenceFile !== undefined && ops.has("consoleLicence")) {
      const state = str(signIn.json, "licence", "state");
      if (state !== "valid") findings.push(`the uploaded licence is ${state} after the upgrade`);
      notes.push(`the licence uploaded to the old server: ${state} on the new one`);
    }
    say("the console signs in");
  }
  say(`reconciler after working the data: ${verify()}`);
  const log = next.log + old.log;
  for (const secret of [
    admin,
    world.sk,
    world.pk,
    world.holds.staff,
    world.holds.buyer,
    world.owner?.password ?? "never",
  ]) {
    if (log.includes(secret)) findings.push("a secret reached a server log");
  }
}

// -------------------------------------------------------------- renderer

/** The page's expression for a seat's status as the map shows it. */
const status = (seat: string | undefined): string =>
  `window.page.map.findObject(${JSON.stringify(seat)})?.status`;

async function renderer(): Promise<void> {
  const url = await database();
  const port = freePort();
  base = `http://localhost:${port}`;
  const admin = hex(24);
  await server(
    newDir!,
    {
      CAVEA_DATABASE_URL: url,
      CAVEA_LISTEN: `127.0.0.1:${port}`,
      CAVEA_ADMIN_KEY: admin,
      CAVEA_PUBLIC_URL: base,
    },
    "new",
  );
  const id = str(
    (await must([201], "POST", "/v1/workspaces", { key: "page", name: "Page" }, { key: admin }))
      .json,
    "id",
  );
  const sk = str(
    (await must([201], "POST", `/v1/workspaces/${id}/keys`, { name: "backend" }, { key: admin }))
      .json,
    "key",
  );
  const pk = str(
    (
      await must(
        [201],
        "POST",
        "/v1/keys",
        { kind: "publishable", name: "page", allowed_origins: null },
        { key: sk },
      )
    ).json,
    "key",
  );
  await must(
    [201],
    "POST",
    "/v1/charts",
    { key: "hall", name: "Hall", document: JSON.parse(cli(newDir!, "gen", "theatre")) },
    { key: sk },
  );
  await must([200, 201], "POST", "/v1/charts/hall/publish", undefined, { key: sk });
  await must([201], "POST", "/v1/events", { key: "night", chart: "hall" }, { key: sk });
  const free = (
    at(
      (
        await must([200], "GET", "/v1/events/night/objects?status=free&limit=2", undefined, {
          key: sk,
        })
      ).json,
      "data",
    ) as Json[]
  ).map((o) => str(o, "label"));

  // The page on an origin of its own, as a customer's would be.
  const page = join(import.meta.dir, "page");
  const site = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      const path = decodeURIComponent(new URL(request.url).pathname);
      const file = path.includes("..")
        ? undefined
        : path.startsWith("/pkg/")
          ? Bun.file(join(rendererDir!, path.slice(5)))
          : Bun.file(join(page, path === "/" ? "index.html" : path));
      return file !== undefined && (await file.exists())
        ? new Response(file)
        : new Response("not found", { status: 404 });
    },
  });
  cleanup.push(async () => void site.stop(true));
  const { chromium, fonts } = await pinned(ROOT);
  const browser = await Browser.launch({
    chromium,
    fonts,
    name: "cavea-upgrade",
    args: ["--host-resolver-rules=MAP localhost 127.0.0.1"],
  });
  cleanup.push(() => browser.close());
  const query = new URLSearchParams({ server: base, key: pk, event: "night" });
  const tab = await browser.open(`http://127.0.0.1:${site.port}/?${query}`);
  await tab.waitFor("window.page?.map || window.page?.failed", 60_000);
  const failed = await tab.evaluate<string | null>("window.page.failed");
  check(failed === null, `the old renderer loaded the new server's event (${failed})`);
  say(`the renderer of ${label} is drawing the new server's event`);

  // A seat the backend holds goes from available to unavailable on the page.
  const first = await tab.evaluate<string | undefined>(status(free[0]));
  check(first === "available", `${free[0]} shows as available first (${first})`);
  const token = str(
    (await must([201], "POST", "/v1/hold-tokens", { event: "night" }, { key: sk })).json,
    "hold_token",
  );
  await must(
    [200],
    "POST",
    "/v1/events/night/hold",
    { hold_token: token, objects: [free[0]] },
    { key: sk },
  );
  await tab.waitFor(`${status(free[0])} === "unavailable"`, 15_000);
  say(`the backend's hold on ${free[0]} reached the page live`);
  // The page holds a seat itself.
  await tab.evaluate(`window.page.map.select([${JSON.stringify(free[1])}])`);
  await tab.waitFor("window.page.selection.holdToken", 15_000);
  const objects = at(
    (
      await must(
        [200],
        "GET",
        `/v1/events/night/objects?label=${encodeURIComponent(free[1]!)}`,
        undefined,
        { key: sk },
      )
    ).json,
    "data",
  ) as Json[];
  check(
    str(objects[0], "status") === "held",
    `the page's hold reached the server (${JSON.stringify(objects[0])})`,
  );
  const errors = [...(await tab.evaluate<string[]>("window.page.errors")), ...browser.errors()];
  check(errors.length === 0, `the page reported no error: ${errors.join("; ")}`);
  say(`the page held ${free[1]} through the new server`);
}

// ------------------------------------------------------------------ main

let code = 0;
try {
  await mkdir(OUT, { recursive: true });
  await (oldDir !== undefined ? upgrade() : renderer());
} catch (error) {
  console.error(`\n${error instanceof Error ? error.message : String(error)}`);
  code = /^failed:|^(GET|POST|PUT|DELETE) /.test(error instanceof Error ? error.message : "")
    ? 1
    : 3;
} finally {
  while (cleanup.length > 0)
    await cleanup.pop()!().catch((e: unknown) => console.error(`clean-up: ${String(e)}`));
}
for (const note of notes) console.log(`note: ${note}`);
for (const finding of findings) console.log(`FINDING: ${finding}`);
if (code === 0 && findings.length > 0) code = 1;
const verdict = code === 0 ? "passed" : code === 1 ? "FAILED" : "could not run";
await writeFile(
  join(OUT, "result.txt"),
  `${label}: ${verdict}\n${notes.map((n) => `note: ${n}\n`).join("")}${findings.map((f) => `FINDING: ${f}\n`).join("")}`,
);
console.log(
  `\nupgrade test (${oldDir !== undefined ? "server" : "renderer"} from ${label}): ${verdict} in ${((performance.now() - started) / 1000).toFixed(0)} s`,
);
process.exit(code);
