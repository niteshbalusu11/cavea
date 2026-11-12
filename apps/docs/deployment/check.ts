// The deployment guide, run as an operator runs it: every block marked
// `check=run:deployment` in `content/deployment/` (the compose file, the
// Caddyfile, start, the setup link, the reconciler, backup and restore) is
// taken from the pages' source with the site's variables filled in, and run
// in a scratch directory with Docker:
//
//   nix develop -c bun apps/docs/deployment/check.ts [--image TAG]
//
// What it proves: Caddy terminates TLS with a certificate made at run time
// (`tls internal`, in place of the page's Let's Encrypt host name); the
// server, behind it with `CAVEA_PUBLIC_URL` on HTTPS, logs a setup link on
// that address; the console's setup, its `__Host-` session cookie and a
// cookie-authenticated change work through the proxy (in headless Chromium,
// by the console's own labels); a buyer's page on another origin, using
// `caveaServer` with the WebSocket transport forced, sees a seat held by a
// backend change live through the proxy; the reconciler is clean; and after
// the page's backup, the database's deletion and the page's restore, a
// booking and an uploaded image are intact and the reconciler clean again.
//
// What differs from a reader's run, on purpose: a compose project name and a
// port of its own, a private subnet no other Docker network uses, `localhost`
// with `tls internal` for the host name, and no port 80. Without --image the
// server image is built from the repository (and removed at the end); the
// seat map is `packages/renderer/dist`, as for the quick-start check.

import { randomBytes } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Browser, pinned } from "../../../tools/chromium.ts";
import { Tab, until } from "../../../tools/e2e/tab.ts";
import { msg } from "../../console/src/i18n/messages.ts";
import { fences, substitute } from "../src/markdown.ts";

const HERE = join(import.meta.dir, "..");
const ROOT = join(HERE, "../..");
const argv = Bun.argv.slice(2);
const givenImage = argv.includes("--image") ? argv[argv.indexOf("--image") + 1] : undefined;
const hex = (n: number): string => randomBytes(n).toString("hex");
const started = performance.now();
const log = (text: string): void =>
  console.log(`[${((performance.now() - started) / 1000).toFixed(1)} s] ${text}`);
const cleanup: Array<() => Promise<void>> = [];

const IMAGE = givenImage ?? `cavea-server:deploy-${hex(4)}`;
const PROJECT = `cavea-deploy-${hex(4)}`;
const DIR = join(ROOT, "target/tmp", PROJECT);
const DIST = join(ROOT, "packages/renderer/dist");
const ASSETS = ["cavea.min.js", "cavea_web_bg.wasm", "glyphs.png"];
const OWNER = { email: "operator@example.com", password: `deployment ${hex(8)}` };

function check(ok: unknown, what: string): asserts ok {
  if (!ok) throw new Error(`deployment check failed: ${what}`);
}

function freePort(): number {
  const server = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } });
  const port = server.port;
  server.stop(true);
  return port;
}

async function run(cmd: string[], cwd = ROOT, input?: string): Promise<string> {
  const proc = Bun.spawn(cmd, {
    cwd,
    stdin: input === undefined ? "ignore" : new TextEncoder().encode(input),
    stdout: "pipe",
    stderr: "pipe",
  });
  const [out, err, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (code !== 0) {
    throw new Error(
      `${cmd.slice(0, 4).join(" ")} … exited ${code}:\n${`${out}\n${err}`.trim().slice(-3000)}`,
    );
  }
  return out;
}

// ------------------------------------------------------------- the pages

interface Block {
  page: string;
  file?: string;
  code: string;
}

async function blocks(): Promise<Block[]> {
  const site = (await Bun.file(join(HERE, "site.json")).json()) as { vars: Record<string, string> };
  const out: Block[] = [];
  for (const page of ["index.md", "monitoring.md", "backups.md"]) {
    const raw = await readFile(join(HERE, "content/deployment", page), "utf8");
    const source = substitute(raw, { ...site.vars, image: IMAGE }, page);
    for (const f of fences(source)) {
      if (f.meta.check !== "run:deployment") continue;
      out.push({ page, code: f.code, ...(f.meta.file === undefined ? {} : { file: f.meta.file }) });
    }
  }
  return out;
}

/** Takes the next block, which must contain `needle`, so every marked block is used, in the pages' order. */
function taker(list: Block[]): { take: (needle: string) => Block; done: () => void } {
  let at = 0;
  return {
    take: (needle) => {
      const b = list[at++];
      check(
        b !== undefined && b.code.includes(needle),
        `block ${at} of the pages contains “${needle}”`,
      );
      return b;
    },
    done: () => check(at === list.length, `every marked block was run (${at} of ${list.length})`),
  };
}

async function freeSubnet(): Promise<string> {
  const ids = (await run(["docker", "network", "ls", "-q"])).trim().split("\n").filter(Boolean);
  const used = await run([
    "docker",
    "network",
    "inspect",
    "-f",
    "{{range .IPAM.Config}}{{.Subnet}} {{end}}",
    ...ids,
  ]);
  for (let tries = 0; tries < 50; tries++) {
    const subnet = `172.31.${100 + Math.floor(Math.random() * 150)}.0/24`;
    if (!used.includes(subnet.slice(0, -5))) return subnet;
  }
  throw new Error("no free 172.31.x.0/24 subnet");
}

// ------------------------------------------------------------- the run

const PORT = freePort();
const BASE = `https://localhost:${PORT}`;
let SUBNET = "";

function localise(text: string): string {
  return text
    .replaceAll("docker compose ", `docker compose -p ${PROJECT} `)
    .replaceAll("cavea_postgres-data", `${PROJECT}_postgres-data`)
    .replaceAll("https://seats.example.com", BASE)
    .replaceAll("172.31.250.0/24", SUBNET)
    .replace(/\n\s*- "80:80"/, "")
    .replace('"443:443"', `"127.0.0.1:${PORT}:443"`)
    .replace("seats.example.com {\n", "localhost {\n\ttls internal\n");
}

async function shell(code: string): Promise<string> {
  return run(["bash", "-c", `set -euo pipefail\n${localise(code)}`], DIR);
}

/** Fetch through Caddy, whose certificate no one trusts but this check. */
function https(path: string, init: RequestInit = {}): Promise<Response> {
  return fetch(`${BASE}${path}`, { ...init, tls: { rejectUnauthorized: false } } as RequestInit);
}

async function json<T>(reply: Response, what: string, status = 200): Promise<T> {
  const text = await reply.text();
  check(reply.status === status, `${what}: ${reply.status} ${text.slice(0, 300)}`);
  return JSON.parse(text) as T;
}

async function main(): Promise<void> {
  const { take, done } = taker(await blocks());
  for (const a of ASSETS) {
    check(
      await Bun.file(join(DIST, a)).exists(),
      `${DIST}/${a} exists (cargo xtask build-web, then bun run build in packages/renderer)`,
    );
  }
  const hadCaddy = (await run(["docker", "image", "ls", "-q", "caddy:2.10-alpine"])).trim() !== "";
  if (givenImage === undefined) {
    log(`building ${IMAGE}`);
    await run(["docker", "buildx", "build", "-f", "deploy/Dockerfile", "-t", IMAGE, "--load", "."]);
    cleanup.push(async () => void (await run(["docker", "rmi", IMAGE]).catch(() => "")));
  }
  if (!hadCaddy)
    cleanup.push(
      async () => void (await run(["docker", "rmi", "caddy:2.10-alpine"]).catch(() => "")),
    );
  SUBNET = await freeSubnet();
  await mkdir(DIR, { recursive: true });
  cleanup.push(async () => {
    await run(["docker", "compose", "-p", PROJECT, "down", "-v", "--remove-orphans"], DIR).catch(
      () => "",
    );
    await rm(DIR, { recursive: true, force: true });
  });
  log(`project ${PROJECT}, ${BASE}, subnet ${SUBNET}`);

  // Running Cavea, steps 1 to 5.
  for (const name of ["compose.yaml", "Caddyfile"]) {
    const b = take(name === "compose.yaml" ? "services:" : "reverse_proxy");
    check(b.file === name, `the block is captioned ${name}`);
    await writeFile(join(DIR, name), localise(b.code));
  }
  await writeFile(join(DIR, ".env"), `POSTGRES_PASSWORD=${hex(32)}\nCAVEA_ADMIN_KEY=${hex(32)}\n`, {
    mode: 0o600,
  });
  await shell(take("up -d --wait").code);
  log("compose up: postgres, server and caddy");
  const link = (await shell(take("setup#token").code)).trim();
  check(
    new RegExp(`^${BASE}/console/setup#token=[0-9a-f]{64}$`).test(link),
    `the setup link is on the public URL: “${link}”`,
  );
  const logs = await run(["docker", "compose", "-p", PROJECT, "logs", "server"], DIR);
  check(!logs.includes("insecure setting"), "the production set-up logs no insecure setting");

  const ready = await https("/readyz");
  check((await ready.text()) === "ready", "/readyz through Caddy");
  check(
    ready.headers.get("strict-transport-security") === "max-age=31536000",
    "HSTS on an HTTPS public URL",
  );

  // The console through the proxy: setup by the link, the cookie, a change.
  const { chromium, fonts } = await pinned(ROOT);
  const browser = await Browser.launch({
    chromium,
    fonts,
    name: "cavea-deploy",
    args: ["--ignore-certificate-errors"],
  });
  cleanup.push(() => browser.close());
  const staff = await Tab.open(browser, "console", 1280, 800);
  staff.allow("GET", /^\/favicon\.ico$/, 404);
  staff.expect("GET", /^\/console\/api\/me$/, 401);
  await staff.goto(link);
  await staff.find({ role: "heading", name: msg("setup.title") }, undefined, 30_000);
  await staff.enter({ role: "textbox", name: msg("setup.email") }, OWNER.email);
  await staff.enter({ role: "textbox", name: msg("setup.password") }, OWNER.password);
  await staff.enter({ role: "textbox", name: msg("setup.confirm") }, OWNER.password);
  await staff.click({ role: "button", name: msg("setup.submit") });
  await staff.find({ role: "heading", name: msg("charts.title") }, undefined, 30_000);
  log("owner created through the proxy; the console's session works");

  const signIn = await https("/console/api/session", {
    method: "POST",
    headers: { origin: BASE, "content-type": "application/json" },
    body: JSON.stringify(OWNER),
  });
  const me = await json<{ csrf_token: string; workspaces: Array<{ id: string }> }>(
    signIn,
    "sign-in",
  );
  const cookie = (signIn.headers.get("set-cookie") ?? "").split(";")[0] ?? "";
  check(
    cookie.startsWith("__Host-") && /;\s*Secure/i.test(signIn.headers.get("set-cookie") ?? ""),
    "a __Host- Secure cookie",
  );
  const staffHeaders = {
    origin: BASE,
    cookie,
    "cavea-csrf-token": me.csrf_token,
    "cavea-workspace": me.workspaces[0]?.id ?? "",
    "content-type": "application/json",
  };
  const secret = (
    await json<{ key: string }>(
      await https("/v1/keys", {
        method: "POST",
        headers: staffHeaders,
        body: JSON.stringify({ kind: "secret", name: "Deployment check" }),
      }),
      "a secret key by a cookie-authenticated change",
      201,
    )
  ).key;
  const auth = { authorization: `Bearer ${secret}`, "content-type": "application/json" };

  // A chart, an event, a publishable key, an image, a booking.
  const theatre = await readFile(
    join(ROOT, "apps/console/src/designer/templates/theatre.json"),
    "utf8",
  );
  await json(
    await https("/v1/charts", {
      method: "POST",
      headers: auth,
      body: `{"key":"grand-hall","name":"Grand Hall","document":${theatre}}`,
    }),
    "chart",
    201,
  );
  await json(
    await https("/v1/charts/grand-hall/publish", { method: "POST", headers: auth }),
    "publish",
    201,
  );
  await json(
    await https("/v1/events", {
      method: "POST",
      headers: auth,
      body: JSON.stringify({ key: "opening-night", name: "Opening Night", chart: "grand-hall" }),
    }),
    "event",
    201,
  );
  const pk = (
    await json<{ key: string }>(
      await https("/v1/keys", {
        method: "POST",
        headers: auth,
        body: JSON.stringify({ kind: "publishable", name: "Buyer page" }),
      }),
      "publishable key",
      201,
    )
  ).key;
  const png = await Bun.file(join(ROOT, "packages/renderer/assets/glyphs.png")).bytes();
  const asset = await json<{ hash: string }>(
    await https("/v1/assets", {
      method: "POST",
      headers: { authorization: `Bearer ${secret}`, "content-type": "image/png" },
      body: png,
    }),
    "image upload",
    201,
  );
  const objects = await json<{ data: Array<{ label: string }> }>(
    await https("/v1/events/opening-night/objects?limit=3", { headers: auth }),
    "objects",
  );
  const [booked = "", held = ""] = objects.data.map((o) => o.label);
  await json(
    await https("/v1/events/opening-night/book", {
      method: "POST",
      headers: auth,
      body: JSON.stringify({ objects: [booked], order_ref: "order-10042" }),
    }),
    "book",
  );
  log(`chart, event, keys, image ${asset.hash.slice(0, 8)}…, ${booked} booked`);

  // A buyer's page on another origin, WebSocket only, through Caddy.
  const page = `<!doctype html><meta charset="utf-8"><div id="map" style="height:500px"></div>
<script src="/cavea/cavea.min.js"></script>
<script>
  Cavea.createSeatMap(document.getElementById("map"), {
    state: Cavea.caveaServer({ baseUrl: ${JSON.stringify(BASE)}, publishableKey: ${JSON.stringify(pk)}, event: "opening-night", transport: "ws" }),
  }).then((map) => { window.seatMap = map; }, (e) => { window.failed = String(e.code); });
</script>`;
  const web = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(request) {
      const path = new URL(request.url).pathname;
      if (path === "/") return new Response(page, { headers: { "content-type": "text/html" } });
      const file = path.replace(/^\/cavea\//, "");
      return ASSETS.includes(file)
        ? new Response(Bun.file(join(DIST, file)))
        : new Response("", { status: 404 });
    },
  });
  cleanup.push(async () => void web.stop(true));
  const buyer = await Tab.open(browser, "buyer", 1000, 700);
  buyer.allow("GET", /^\/favicon\.ico$/, 404);
  await buyer.goto(`http://127.0.0.1:${web.port}/`);
  await until(
    "the buyer's map to be ready",
    async () => {
      const failed = await buyer.evaluate<string | null>("window.failed ?? null");
      check(failed === null, `the buyer's map failed: ${failed}`);
      return buyer.evaluate<boolean>("window.seatMap !== undefined");
    },
    30_000,
  );
  const status = (label: string): Promise<string> =>
    buyer.evaluate<string>(`window.seatMap.findObject(${JSON.stringify(label)})?.status ?? "none"`);
  check(
    (await status(booked)) === "unavailable",
    "the booked seat is unavailable on the buyer's map",
  );
  check((await status(held)) === "available", "the other seat is available before the hold");
  const token = await json<{ hold_token: string }>(
    await https("/v1/hold-tokens", {
      method: "POST",
      headers: auth,
      body: JSON.stringify({ event: "opening-night" }),
    }),
    "hold token",
    201,
  );
  await json(
    await https("/v1/events/opening-night/hold", {
      method: "POST",
      headers: auth,
      body: JSON.stringify({ hold_token: token.hold_token, objects: [held] }),
    }),
    "hold",
  );
  await until(
    `${held} to turn unavailable live on the buyer's map`,
    async () => (await status(held)) === "unavailable",
    15_000,
  );
  const sockets = [...buyer.sockets.values()].filter((s) =>
    s.url.startsWith(`wss://localhost:${PORT}/v1/public/rt`),
  );
  check(
    sockets.some((s) => s.status === 101 && s.frames > 1),
    `a WebSocket upgraded through Caddy and received frames: ${JSON.stringify(sockets)}`,
  );
  log(`${held} held by the backend reached the buyer's map over wss:// through Caddy`);

  // The reconciler, from the monitoring page.
  const verify = take("cavea-server verify");
  await shell(verify.code);
  log("reconciler clean");

  // Backups: dump, list, delete, restore, start, verify.
  await shell(take("pg_dump").code);
  const dump = Bun.file(join(DIR, "cavea-backup.dump"));
  check((await dump.size) > 10_000, `a backup was written (${await dump.size} bytes)`);
  const list = await shell(take("pg_restore --list").code);
  check(
    list.includes("TABLE DATA public assets") && list.includes("TABLE DATA public object_state"),
    "the backup lists the assets and the seat state",
  );
  await shell(take("docker volume rm").code);
  const volumes = await run([
    "docker",
    "volume",
    "ls",
    "-q",
    "--filter",
    `name=${PROJECT}_postgres-data`,
  ]);
  check(volumes.trim() === "", "the database volume is gone");
  await shell(take("up -d --wait postgres").code);
  await shell(take("pg_restore -U cavea").code);
  await shell(take("up -d --wait").code);
  await shell(take("cavea-server verify").code);
  log("restored; reconciler clean");
  const after = await json<{ data: Array<{ label: string; status: string; order_ref?: string }> }>(
    await https(`/v1/events/opening-night/objects?label=${encodeURIComponent(booked)}`, {
      headers: auth,
    }),
    "the booked seat after the restore",
  );
  check(
    after.data[0]?.status === "booked" && after.data[0]?.order_ref === "order-10042",
    `the booking survived: ${JSON.stringify(after.data)}`,
  );
  const image = await https(`/v1/public/assets/${asset.hash}`);
  check(
    image.status === 200 && Buffer.from(await image.arrayBuffer()).equals(Buffer.from(png)),
    "the uploaded image survived, byte for byte",
  );
  log("the booking and the uploaded image are intact");
  done();
}

try {
  await main();
  console.log(`deployment check passed in ${((performance.now() - started) / 1000).toFixed(0)} s`);
} finally {
  for (const c of cleanup.toReversed()) await c();
}
