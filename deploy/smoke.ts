// The image end to end (08 §14, 13 §13): build it, run it with its own
// PostgreSQL under a compose project of its own on a free port, create the
// owner from the setup link in the log, publish a chart, create an event,
// hold and book a seat as buyer and backend, fetch the console, reconcile,
// stop within the grace period, start again and find the data. Everything it
// creates is removed at the end (the image too, unless given with --image).
//
//   nix develop -c bun deploy/smoke.ts [--image TAG]

const root = new URL("..", import.meta.url).pathname;
const args = Bun.argv.slice(2);
const given = args[args.indexOf("--image") + 1];
const image = args.includes("--image") && given ? given : `cavea-server:smoke-${hex(4)}`;
const project = `cavea-smoke-${hex(4)}`;
const port = freePort();
const base = `http://127.0.0.1:${port}`;
const password = hex(16);
const env = {
  ...process.env,
  POSTGRES_PASSWORD: hex(16),
  CAVEA_ADMIN_KEY: hex(24),
  CAVEA_PORT: String(port),
  CAVEA_IMAGE: image,
  CAVEA_PUBLIC_URL: `http://localhost:${port}`,
};
const compose = ["docker", "compose", "-p", project, "-f", `${root}deploy/compose.server.yaml`];
const started = performance.now();

function hex(bytes: number): string {
  return Buffer.from(crypto.getRandomValues(new Uint8Array(bytes))).toString("hex");
}

function freePort(): number {
  const server = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } });
  const free = server.port;
  server.stop(true);
  return free;
}

function step(text: string): void {
  console.log(`[${((performance.now() - started) / 1000).toFixed(1)} s] ${text}`);
}

function check(ok: unknown, text: string): void {
  if (!ok) {
    throw new Error(`failed: ${text}`);
  }
}

function sh(command: string[], options: { quiet?: boolean } = {}): string {
  const run = Bun.spawnSync(command, {
    cwd: root,
    env,
    stderr: options.quiet ? "pipe" : "inherit",
  });
  if (run.exitCode !== 0) {
    throw new Error(`${command.slice(0, 6).join(" ")} … exited with ${run.exitCode}`);
  }
  return run.stdout.toString();
}

async function until<T>(
  what: string,
  seconds: number,
  probe: () => Promise<T | undefined>,
): Promise<T> {
  const deadline = performance.now() + seconds * 1000;
  for (;;) {
    const value = await probe().catch(() => undefined);
    if (value !== undefined) {
      return value;
    }
    check(performance.now() < deadline, `${what} within ${seconds} s`);
    await Bun.sleep(250);
  }
}

const ready = () =>
  until("/readyz", 60, async () =>
    (await fetch(`${base}/readyz`)).status === 200 ? true : undefined,
  );

let cookie = "";
let csrf = "";
let workspace = "";

/** A field of a JSON reply, by its path. */
function at(value: unknown, ...path: (string | number)[]): unknown {
  return path.reduce<unknown>((v, k) => (v as Record<string | number, unknown> | null)?.[k], value);
}

/** A JSON request; `as` is a bearer key, else the console session. */
async function api(
  method: string,
  path: string,
  body?: unknown,
  as?: string,
): Promise<[number, unknown]> {
  const headers: Record<string, string> = { origin: base };
  if (body !== undefined) {
    headers["content-type"] = "application/json";
  }
  if (as) {
    headers.authorization = `Bearer ${as}`;
  } else if (cookie) {
    Object.assign(headers, { cookie, "cavea-csrf-token": csrf, "cavea-workspace": workspace });
  }
  const response = await fetch(base + path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const set = response.headers.get("set-cookie");
  if (set) {
    cookie = set.split(";")[0] ?? "";
  }
  const text = await response.text();
  return [response.status, text ? JSON.parse(text) : null];
}

function signedIn(me: unknown): void {
  csrf = String(at(me, "csrf_token"));
  workspace = String(at(me, "workspaces", 0, "id"));
}

async function main(): Promise<void> {
  if (!args.includes("--image")) {
    step(`building ${image}`);
    sh(["docker", "buildx", "build", "-f", "deploy/Dockerfile", "-t", image, "--load", "."]);
  }
  step(`starting ${project} on port ${port}`);
  sh([...compose, "up", "-d", "--wait"]);
  await ready();
  const id = sh([...compose, "ps", "-q", "server"]).trim();
  const inspect = JSON.parse(sh(["docker", "inspect", id]))[0];
  check(inspect.Config.User === "nonroot:nonroot", "runs as nonroot");
  check(inspect.HostConfig.ReadonlyRootfs, "read-only root filesystem");
  await until("healthy", 60, async () =>
    sh(["docker", "inspect", "-f", "{{.State.Health.Status}}", id]).trim() === "healthy"
      ? true
      : undefined,
  );
  step("ready, healthy, nonroot, read-only");

  const link = sh([...compose, "logs", "server"]).match(/setup_link"?[:=]"?([^"\s]+)/)?.[1];
  check(link?.startsWith(`http://localhost:${port}/console/setup#token=`), "setup link logged");
  const token = link!.split("#token=")[1];
  const page = await fetch(`${base}/console/setup`, { headers: { accept: "text/html" } });
  check(
    page.status === 200 && (await page.text()).includes('<div id="root">'),
    "setup link lands on the console",
  );
  let [status, me] = await api("POST", "/console/api/setup", {
    token,
    email: "owner@example.org",
    password,
  });
  check(status === 201, `setup ${status}`);
  signedIn(me);
  [status] = await api("DELETE", "/console/api/session");
  check(status === 204, "sign-out");
  [status, me] = await api("POST", "/console/api/session", {
    email: "owner@example.org",
    password,
  });
  check(status === 200, `sign-in ${status}`);
  signedIn(me);
  step("owner created from the setup link; signed out and in");

  const document = JSON.parse(
    sh(["cargo", "run", "--release", "-q", "-p", "cavea-cli", "--", "gen", "theatre"]),
  );
  [status] = await api("POST", "/v1/charts", { key: "smoke", name: "Smoke", document });
  check(status === 201, `chart ${status}`);
  [status] = await api("POST", "/v1/charts/smoke/publish");
  check(status === 201, `publish ${status}`);
  [status] = await api("POST", "/v1/events", { key: "smoke-night", chart: "smoke" });
  check(status === 201, `event ${status}`);
  const [, secret] = await api("POST", "/v1/keys", { kind: "secret", name: "smoke backend" });
  const [, publishable] = await api("POST", "/v1/keys", {
    kind: "publishable",
    name: "smoke buyers",
  });
  step("chart published, event created, keys made by the session");

  const sk = String(at(secret, "key"));
  const pk = String(at(publishable, "key"));
  const [, objects] = await api(
    "GET",
    "/v1/events/smoke-night/objects?status=free&limit=1",
    undefined,
    sk,
  );
  const label = String(at(objects, "data", 0, "label"));
  const [, held] = await api("POST", "/v1/public/hold-tokens", { event: "smoke-night" }, pk);
  const hold = String(at(held, "hold_token"));
  [status] = await api(
    "POST",
    "/v1/public/events/smoke-night/hold",
    { hold_token: hold, objects: [label] },
    pk,
  );
  check(status === 200, `hold ${status}`);
  [status] = await api(
    "POST",
    "/v1/events/smoke-night/book",
    { hold_token: hold, order_ref: "smoke-1" },
    sk,
  );
  check(status === 200, `book ${status}`);
  let [, event] = await api("GET", "/v1/events/smoke-night", undefined, sk);
  check(at(event, "counts", "booked") === 1, "one seat booked");
  step(`seat ${label} held by the buyer and booked by the backend`);

  const get = (path: string) =>
    fetch(base + path, {
      headers: { accept: "text/html", "accept-encoding": "br" },
      decompress: false,
    } as RequestInit);
  const index = await get("/");
  const html = await (await fetch(`${base}/`)).text();
  const script = html.match(/\/assets\/index-[\w-]+\.js/)?.[0];
  const code = await (await fetch(base + script)).text();
  const wasm = code.match(/cavea_designer_bg-[\w-]+\.wasm/)?.[0];
  for (const [path, response, type, cache] of [
    ["/", index, "text/html; charset=utf-8", "no-cache"],
    [
      script,
      await get(script!),
      "text/javascript; charset=utf-8",
      "public, max-age=31536000, immutable",
    ],
    [
      `/assets/${wasm}`,
      await get(`/assets/${wasm}`),
      "application/wasm",
      "public, max-age=31536000, immutable",
    ],
  ] as const) {
    const h = response.headers;
    console.log(
      `  ${path}: ${response.status} ${h.get("content-type")}, ${h.get("content-encoding")}, ${h.get("content-length")} B, ${h.get("cache-control")}, etag ${h.get("etag")}, vary ${h.get("vary")}, ${h.get("x-content-type-options")}`,
    );
    check(
      response.status === 200 && h.get("content-type") === type && h.get("cache-control") === cache,
      path,
    );
    check(
      h.get("content-encoding") === "br" && h.get("vary") === "Accept-Encoding",
      `${path} brotli`,
    );
  }
  step("console served: index.html, a hashed script, the designer's WASM");

  const verify = sh([
    ...compose,
    "exec",
    "-T",
    "server",
    "/usr/local/bin/cavea-server",
    "verify",
    "--all",
  ]);
  console.log(verify.trim().replace(/^/gm, "  "));
  step("reconciler clean");

  const stopping = performance.now();
  sh([...compose, "stop", "server"], { quiet: true });
  const seconds = (performance.now() - stopping) / 1000;
  const exit = sh(["docker", "inspect", "-f", "{{.State.ExitCode}}", id]).trim();
  check(exit === "0" && seconds < 30, `stopped with ${exit} in ${seconds.toFixed(1)} s`);
  const logs = sh([...compose, "logs", "server"]);
  check(logs.includes('"stopped"'), "logged its stop");
  step(`stopped cleanly in ${seconds.toFixed(1)} s (exit 0)`);
  sh([...compose, "start", "server"], { quiet: true });
  await ready();
  [, event] = await api("GET", "/v1/events/smoke-night", undefined, sk);
  check(at(event, "counts", "booked") === 1, "the booking survived the restart");
  step("started again: the booking is there");

  const all = sh([...compose, "logs", "server"]);
  for (const kept of [
    password,
    sk,
    pk,
    hold,
    env.CAVEA_ADMIN_KEY,
    env.POSTGRES_PASSWORD,
    "owner@example.org",
  ]) {
    check(!all.includes(kept), "no secret or email in the log");
  }
  const sched = all.includes("SCHED_IDLE") ? "refused (logged)" : "applied, nothing logged";
  step(`log holds no secret but the setup link; SCHED_IDLE ${sched}`);
}

try {
  await main();
  console.log("smoke test passed");
} finally {
  sh([...compose, "down", "-v", "--remove-orphans"], { quiet: true });
  if (!args.includes("--image")) {
    sh(["docker", "rmi", image], { quiet: true });
  }
}
