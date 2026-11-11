// The compatibility runner (docs/13-testing-benchmarks.md §8): builds and
// packs `@cavea/renderer` and `@cavea/react` as a customer gets them, builds
// each fixture against the tarballs, serves the result and loads it in
// headless Chromium (WebGL2 on SwiftShader: software rendering) over the
// DevTools protocol, then prints a table. Run from the repository root after
// `cargo xtask build-web`:
//
//   bun fixtures/run.ts [check…]
//
// Checks: script-tag, vite, vite-dev, webpack, rollup, esbuild, next,
// react18, csp-custom, csp-server, csp-cdn; none named runs them all. Fixtures are
// copied to `target/tmp/compat/<name>` and installed there with bun from their
// own lockfiles (never into the workspace); builds run under Node, as most
// customers run them. Chromium and Node come from PATH or, failing that, from
// `nix build nixpkgs#chromium` and `nixpkgs#nodejs`. `csp-server` needs the
// PostgreSQL of `deploy/compose.yaml` and builds `cavea-server`; it creates
// and drops a database of its own. Everything started is stopped by process
// id at the end.
import { createHash, randomBytes } from "node:crypto";
import { cp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { join, relative } from "node:path";

const ROOT = join(import.meta.dir, "..");
const WORK = join(ROOT, "target/tmp/compat");
const FIXTURES = import.meta.dir;
const PKGS = join(WORK, "pkgs");
const DIST = join(ROOT, "packages/renderer/dist");
const ALL =
  "script-tag vite vite-dev webpack rollup esbuild next react18 csp-custom csp-server csp-cdn";

interface Row {
  check: string;
  tool: string;
  ok: boolean;
  detail: string;
}
const rows: Row[] = [];
const cleanup: Array<() => Promise<void> | void> = [];

// ---------------------------------------------------------------- processes

/** Runs `cmd`, returning its output; throws with the output's tail on failure. */
async function sh(
  cmd: string[],
  options: { cwd?: string; env?: Record<string, string> } = {},
): Promise<string> {
  const proc = Bun.spawn(cmd, {
    cwd: options.cwd ?? ROOT,
    env: { ...process.env, ...options.env },
    stdout: "pipe",
    stderr: "pipe",
  });
  const [out, err, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (code !== 0) {
    const tail = `${out}\n${err}`.trim().split("\n").slice(-15).join("\n");
    throw new Error(`${cmd.join(" ")} exited with ${code}:\n${tail}`);
  }
  return out;
}

/** A long-running process, stopped by its id at the end; its output is read as it comes. */
interface Running {
  proc: Bun.Subprocess<"ignore", "pipe", "pipe">;
  /** Everything written to stdout and stderr so far (the last 64 KiB). */
  output: string;
}
function start(
  cmd: string[],
  options: { cwd?: string; env?: Record<string, string> } = {},
): Running {
  const proc = Bun.spawn(cmd, {
    cwd: options.cwd ?? ROOT,
    env: { ...process.env, ...options.env },
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const running: Running = { proc, output: "" };
  // Drained, so a chatty process never blocks on a full pipe.
  for (const stream of [proc.stdout, proc.stderr]) {
    void (async () => {
      const decoder = new TextDecoder();
      for await (const chunk of stream) {
        running.output = (running.output + decoder.decode(chunk)).slice(-65_536);
      }
    })();
  }
  cleanup.push(async () => {
    proc.kill();
    await proc.exited;
  });
  return running;
}

/** Waits until `running`'s output matches `pattern`, or fails after `ms`. */
async function waitFor(running: Running, pattern: RegExp, ms: number): Promise<RegExpMatchArray> {
  for (const deadline = Date.now() + ms; Date.now() < deadline; await Bun.sleep(100)) {
    const match = running.output.match(pattern);
    if (match !== null) {
      return match;
    }
    if (running.proc.exitCode !== null) {
      break;
    }
  }
  throw new Error(`waiting for ${pattern}:\n${running.output.slice(-1000)}`);
}

/** A tool from PATH, or from nixpkgs. */
async function tool(name: string, attr: string): Promise<string> {
  const found = Bun.which(name);
  if (found !== null) {
    return found;
  }
  const out = await sh(["nix", "build", "--no-link", "--print-out-paths", `nixpkgs#${attr}`]);
  return join(out.trim().split("\n").at(-1) ?? "", "bin", name);
}

const freePort = (): number => {
  const server = Bun.serve({ port: 0, fetch: () => new Response() });
  const port = server.port ?? 0;
  server.stop(true);
  return port;
};

// ------------------------------------------------------------------ serving

interface Route {
  /** URL path prefix, ending in `/`. */
  prefix: string;
  dir: string;
}

/** A static server over `routes`, every answer with `headers`; `html` rewrites HTML. */
function serve(
  routes: Route[],
  options: {
    host?: string;
    headers?: (path: string) => Record<string, string>;
    html?: (text: string) => string;
  } = {},
): string {
  const server = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    async fetch(request) {
      let path = decodeURIComponent(new URL(request.url).pathname);
      if (path.endsWith("/")) {
        path += "index.html";
      }
      const route = routes.find((r) => path.startsWith(r.prefix));
      const file =
        route === undefined
          ? undefined
          : Bun.file(join(route.dir, path.slice(route.prefix.length)));
      const headers = options.headers?.(path) ?? {};
      if (file === undefined || path.includes("..") || !(await file.exists())) {
        return new Response("not found", { status: 404, headers });
      }
      if (path.endsWith(".html") && options.html !== undefined) {
        return new Response(options.html(await file.text()), {
          headers: { "content-type": "text/html", ...headers },
        });
      }
      return new Response(file, { headers });
    },
  });
  cleanup.push(() => server.stop(true));
  return `http://${options.host ?? "127.0.0.1"}:${server.port}`;
}

// ------------------------------------------------------------------ browser

interface Visit {
  firstFrame?: number;
  failed?: string;
  errors: string[];
  /** Responses: path (or URL, for another origin) and status. */
  fetched: Array<{ url: string; status: number }>;
  blocked: string[];
  wsFrames: number;
  sseMessages: number;
  logs: string[];
}

type Message = {
  id?: number;
  method?: string;
  params?: Record<string, unknown>;
  sessionId?: string;
  result?: Record<string, unknown>;
  error?: { message: string };
};

class Browser {
  private id = 0;
  private readonly pending = new Map<number, (m: Message) => void>();
  private readonly listeners = new Set<(m: Message) => void>();
  private constructor(private readonly ws: WebSocket) {
    ws.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as Message;
      if (message.id !== undefined) {
        this.pending.get(message.id)?.(message);
        this.pending.delete(message.id);
      } else {
        for (const listener of this.listeners) {
          listener(message);
        }
      }
    });
  }

  static async launch(): Promise<Browser> {
    const chromium = await tool("chromium", "chromium");
    // Chromium's socket path must be short; the worktree's is not.
    const sockets = `/tmp/cavea-compat-${process.pid}`;
    await mkdir(sockets, { recursive: true });
    cleanup.push(() => rm(sockets, { recursive: true, force: true }));
    const proc = start(
      [
        chromium,
        "--headless",
        "--no-sandbox",
        "--remote-debugging-port=0",
        `--user-data-dir=${join(WORK, "profile")}`,
        "--use-angle=swiftshader",
        "--enable-unsafe-swiftshader",
        "--no-first-run",
        "about:blank",
      ],
      { env: { TMPDIR: sockets } },
    );
    const [, url] = await waitFor(proc, /DevTools listening on (ws:\/\/\S+)/, 30_000);
    const ws = new WebSocket(url ?? "");
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve);
      ws.addEventListener("error", reject);
    });
    return new Browser(ws);
  }

  send(
    method: string,
    params: Record<string, unknown> = {},
    sessionId?: string,
  ): Promise<Record<string, unknown>> {
    const id = ++this.id;
    this.ws.send(
      JSON.stringify({ id, method, params, ...(sessionId === undefined ? {} : { sessionId }) }),
    );
    return new Promise((resolve, reject) => {
      this.pending.set(id, (m) =>
        m.error === undefined
          ? resolve(m.result ?? {})
          : reject(new Error(`${method}: ${m.error.message}`)),
      );
    });
  }

  version(): Promise<string> {
    return this.send("Browser.getVersion").then((v) => String(v["product"]));
  }

  /**
   * Opens `url` in a fresh context (a cold cache) and waits until the page's
   * `window.compat` has `firstFrame` or `failed`, or `ms` pass.
   */
  async visit(url: string, ms = 30_000): Promise<Visit> {
    const { browserContextId } = await this.send("Target.createBrowserContext");
    const { targetId } = await this.send("Target.createTarget", {
      url: "about:blank",
      browserContextId,
    });
    const { sessionId } = await this.send("Target.attachToTarget", { targetId, flatten: true });
    const session = String(sessionId);
    const origin = new URL(url).origin;
    const visit: Visit = {
      errors: [],
      fetched: [],
      blocked: [],
      wsFrames: 0,
      sseMessages: 0,
      logs: [],
    };
    const urls = new Map<string, string>();
    const short = (u: string): string => (u.startsWith(origin) ? u.slice(origin.length) : u);
    const listener = (m: Message): void => {
      if (m.sessionId !== session || m.params === undefined) {
        return;
      }
      const p = m.params as Record<string, unknown>;
      const id = String(p["requestId"]);
      switch (m.method) {
        case "Network.requestWillBeSent":
          urls.set(id, (p["request"] as { url: string }).url);
          break;
        case "Network.responseReceived": {
          const response = p["response"] as { url: string; status: number };
          visit.fetched.push({ url: short(response.url), status: response.status });
          break;
        }
        case "Network.loadingFailed":
          visit.blocked.push(
            `${short(urls.get(id) ?? "?")} ${p["blockedReason"] ?? p["errorText"]}`,
          );
          break;
        case "Network.webSocketFrameReceived":
          visit.wsFrames++;
          break;
        case "Network.eventSourceMessageReceived":
          visit.sseMessages++;
          break;
        case "Runtime.exceptionThrown":
          visit.logs.push(`exception: ${JSON.stringify(p["exceptionDetails"]).slice(0, 300)}`);
          break;
        case "Runtime.consoleAPICalled":
          if (p["type"] === "error" || p["type"] === "warning") {
            const args = p["args"] as Array<{ value?: unknown; description?: string }>;
            visit.logs.push(
              `${p["type"]}: ${args
                .map((a) => a.value ?? a.description)
                .join(" ")
                .slice(0, 300)}`,
            );
          }
          break;
        default:
      }
    };
    this.listeners.add(listener);
    try {
      await this.send("Network.enable", {}, session);
      await this.send("Runtime.enable", {}, session);
      await this.send("Page.enable", {}, session);
      await this.send("Page.navigate", { url }, session);
      const deadline = Date.now() + ms;
      while (Date.now() < deadline) {
        await Bun.sleep(200);
        const { result } = await this.send(
          "Runtime.evaluate",
          { expression: "JSON.stringify(window.compat ?? null)", returnByValue: true },
          session,
        );
        const compat = JSON.parse(
          String((result as { value?: string }).value ?? "null"),
        ) as Partial<Visit> | null;
        if (compat !== null) {
          visit.errors = compat.errors ?? [];
          if (compat.firstFrame !== undefined || compat.failed !== undefined) {
            Object.assign(visit, { firstFrame: compat.firstFrame, failed: compat.failed });
            // Let a realtime stream deliver its first frames (or fall back: 05 §4.1).
            await Bun.sleep(3_000);
            break;
          }
        }
      }
      return visit;
    } finally {
      this.listeners.delete(listener);
      await this.send("Target.disposeBrowserContext", { browserContextId });
    }
  }
}

let browser: Browser | undefined;
const chromium = async (): Promise<Browser> => {
  browser ??= await Browser.launch();
  return browser;
};

/** The map drew, and nothing went to `onError` or the console's errors. */
const drew = (v: Visit): boolean => v.firstFrame !== undefined && v.errors.length === 0;
const outcome = (v: Visit): string =>
  v.firstFrame !== undefined
    ? `first frame at ${Math.round(v.firstFrame)} ms${v.errors.length > 0 ? `, errors ${v.errors.join(" ")}` : ""}`
    : `no frame: ${v.failed ?? "timed out"}${v.errors.length > 0 ? `; errors ${v.errors.join(" ")}` : ""}${v.blocked.length > 0 ? `; blocked ${v.blocked.join(", ")}` : ""}${v.logs.length > 0 ? `; ${v.logs.slice(0, 3).join("; ")}` : ""}`;
/** Where the WASM and the atlas came from. */
const assets = (v: Visit): string =>
  v.fetched
    .filter((f) => /\.(wasm|png)$/.test(f.url))
    .map((f) => `${f.url} ${f.status}`)
    .join(", ");
const loadedFrom = (v: Visit, wasm: string, atlas: string): boolean =>
  v.fetched.some((f) => f.url === wasm && f.status === 200) &&
  v.fetched.some((f) => f.url === atlas && f.status === 200);

// ---------------------------------------------------------------- preparing

let chartFile = "";

/** Builds and packs both packages, and compiles the theatre preset. */
async function prepare(): Promise<void> {
  try {
    await readFile(join(ROOT, "packages/renderer/wasm/cavea_web_bg.wasm"));
  } catch {
    throw new Error("run `cargo xtask build-web` first");
  }
  await mkdir(PKGS, { recursive: true });
  for (const pkg of ["renderer", "react"]) {
    await sh(["bun", "run", "build"], { cwd: join(ROOT, "packages", pkg) });
    await sh(["bun", "pm", "pack", "--quiet", "--destination", PKGS], {
      cwd: join(ROOT, "packages", pkg, "dist"),
    });
  }
  const cargo = { CARGO_BUILD_JOBS: "8" };
  await sh(["cargo", "build", "-q", "-p", "cavea-cli"], { env: cargo });
  const cli = join(ROOT, "target/debug/cavea");
  const doc = join(WORK, "theatre.json");
  await sh([cli, "gen", "theatre", "-o", doc]);
  const out = join(WORK, "chart");
  await rm(out, { recursive: true, force: true });
  await sh([cli, "compile", doc, "-o", out]);
  const name = (await readdir(out)).find((f) => f.endsWith(".ccf") && !f.endsWith(".full.ccf"));
  chartFile = join(out, name ?? "missing.ccf");
}

/**
 * Copies fixture `name` to `target/tmp/compat/<name>` (keeping an earlier
 * install), installs its lockfile with bun and the two tarballs as a package
 * manager would (unpacked into `node_modules/@cavea/`). Returns the copy.
 */
async function install(name: string): Promise<string> {
  const dir = join(WORK, name);
  await mkdir(join(dir, "src"), { recursive: true });
  await cp(join(FIXTURES, name), dir, { recursive: true });
  if (name !== "next" && name !== "react18") {
    await cp(join(FIXTURES, "shared/main.js"), join(dir, "src/main.js"));
  }
  try {
    await sh(["bun", "install", "--frozen-lockfile"], { cwd: dir });
  } catch (error) {
    throw new Error(
      `installing the ${name} fixture failed (the network is needed the first time): ${String(error)}`,
      { cause: error },
    );
  }
  for (const pkg of ["renderer", "react"]) {
    const into = join(dir, "node_modules/@cavea", pkg);
    await rm(into, { recursive: true, force: true });
    await mkdir(into, { recursive: true });
    await sh([
      "tar",
      "-xzf",
      join(PKGS, `cavea-${pkg}-0.0.0.tgz`),
      "-C",
      into,
      "--strip-components=1",
    ]);
  }
  return dir;
}

/** `PATH` with Node first, so the bundlers' `#!/usr/bin/env node` run on Node. */
let nodePath = "";
async function node(dir: string): Promise<Record<string, string>> {
  nodePath ||= join(await tool("node", "nodejs"), "..");
  return { PATH: `${join(dir, "node_modules/.bin")}:${nodePath}:${process.env["PATH"] ?? ""}` };
}

/** A fixture's `package.json` script, run as `sh -c` so that its tools run on Node. */
const script = async (dir: string, name: string): Promise<string> =>
  (
    JSON.parse(await readFile(join(dir, "package.json"), "utf8")) as {
      scripts: Record<string, string>;
    }
  ).scripts[name] ?? "";

const versionOf = async (dir: string, pkg: string): Promise<string> =>
  `${pkg} ${(JSON.parse(await readFile(join(dir, "node_modules", pkg, "package.json"), "utf8")) as { version: string }).version}`;

/** Files under `dir`, relative to it. */
async function files(dir: string): Promise<string[]> {
  return (await readdir(dir, { recursive: true, withFileTypes: true }))
    .filter((e) => e.isFile())
    .map((e) => relative(dir, join(e.parentPath, e.name)));
}

/** The WASM is a file of its own in the output, and no script carries it inlined (base64 or a data URL). */
async function notInlined(out: string): Promise<string | null> {
  const all = await files(out);
  if (!all.some((f) => f.endsWith(".wasm"))) {
    return "no .wasm file in the output";
  }
  for (const f of all.filter((name) => /\.m?js$/.test(name))) {
    const text = await readFile(join(out, f), "utf8");
    if (text.includes("AGFzbQ") || text.includes("data:application/wasm")) {
      return `${f} inlines the WASM`;
    }
  }
  return null;
}

// ------------------------------------------------------------------- checks

async function bundler(name: string, pkg: string, out: string, html: boolean): Promise<void> {
  const dir = await install(name);
  const env = await node(dir);
  await rm(join(dir, out), { recursive: true, force: true });
  await sh(["sh", "-c", await script(dir, "build")], { cwd: dir, env });
  const dist = join(dir, out);
  if (html) {
    await cp(join(dir, "index.html"), join(dist, "index.html"));
  }
  await cp(chartFile, join(dist, "chart.ccf"));
  const inlined = await notInlined(dist);
  const v = await (await chromium()).visit(`${serve([{ prefix: "/", dir: dist }])}/`);
  rows.push({
    check: name,
    tool: await versionOf(dir, pkg),
    ok: drew(v) && inlined === null,
    detail: `${outcome(v)}; ${inlined ?? "WASM a file of its own"}; ${assets(v)}`,
  });
}

async function viteDev(): Promise<void> {
  const dir = await install("vite");
  await cp(chartFile, join(dir, "chart.ccf"));
  const port = freePort();
  const proc = start(["vite", "--port", String(port), "--strictPort", "--host", "127.0.0.1"], {
    cwd: dir,
    env: await node(dir),
  });
  await waitFor(proc, /ready in|Local:/, 60_000);
  const v = await (await chromium()).visit(`http://127.0.0.1:${port}/`);
  rows.push({
    check: "vite-dev",
    tool: await versionOf(dir, "vite"),
    ok: drew(v),
    detail: `vite dev server: ${outcome(v)}; ${assets(v)}`,
  });
}

async function next(): Promise<void> {
  const dir = await install("next");
  const env = { ...(await node(dir)), NEXT_TELEMETRY_DISABLED: "1" };
  await rm(join(dir, ".next"), { recursive: true, force: true });
  await mkdir(join(dir, "public"), { recursive: true });
  await cp(chartFile, join(dir, "public/chart.ccf"));
  // Prerendering the pages is the server render: it fails if an import touches the DOM.
  await sh(["next", "build"], { cwd: dir, env });
  const port = freePort();
  const proc = start(["next", "start", "-p", String(port), "-H", "127.0.0.1"], { cwd: dir, env });
  await waitFor(proc, /Ready|started server/i, 60_000);
  const base = `http://127.0.0.1:${port}`;
  for (const page of ["/", "/direct"]) {
    const html = await (await fetch(`${base}${page}`)).text();
    const container = html.includes('id="map"');
    const v = await (await chromium()).visit(`${base}${page}`);
    rows.push({
      check: `next ${page}`,
      tool: await versionOf(dir, "next"),
      ok: container && drew(v),
      detail: `${page === "/" ? "<SeatMap> in a client component" : "dynamic import()"}: built and server-rendered (${container ? "container in the HTML" : "no container in the HTML"}); hydrated: ${outcome(v)}; ${assets(v)}`,
    });
  }
}

/** The wrapper's own tests (`packages/react/src/index.test.ts`), under React 18. */
async function react18(): Promise<void> {
  const dir = await install("react18");
  await cp(join(ROOT, "packages/react/src"), join(dir, "src"), { recursive: true });
  const out = await sh(["sh", "-c", "bun test src 2>&1"], { cwd: dir }).then(
    (o) => ({ ok: true, text: o }),
    (error: unknown) => ({ ok: false, text: String(error) }),
  );
  const tests = /(\d+) pass[\s\S]*?(\d+) fail/.exec(out.text);
  rows.push({
    check: "react18",
    tool: await versionOf(dir, "react-dom"),
    ok: out.ok,
    detail: `the wrapper's tests under React 18 (Strict Mode, setters, callbacks, unmount while loading, server render): ${tests === null ? out.text.slice(-300) : `${tests[1]} pass, ${tests[2]} fail`}`,
  });
}

const integrity = async (file: string): Promise<string> =>
  `sha384-${createHash("sha384")
    .update(await readFile(file))
    .digest("base64")}`;

/** The script-tag page, its script at `script` and assets at `base` (if given). */
async function scriptTag(): Promise<void> {
  const page = join(WORK, "script-tag");
  await rm(page, { recursive: true, force: true });
  await cp(join(FIXTURES, "script-tag"), page, { recursive: true });
  await cp(chartFile, join(page, "chart.ccf"));
  const sri = await integrity(join(DIST, "cavea.min.js"));
  const scriptOnly = join(WORK, "script-only");
  await mkdir(scriptOnly, { recursive: true });
  await cp(join(DIST, "cavea.min.js"), join(scriptOnly, "cavea.min.js"));
  for (const [check, src, query, wasm, atlas] of [
    ["script-tag", "/lib/v0/cavea.min.js", "", "/lib/v0/cavea_web_bg.wasm", "/lib/v0/glyphs.png"],
    [
      "script-tag, assets.baseUrl",
      "/js/cavea.min.js",
      "?base=/static/cavea/",
      "/static/cavea/cavea_web_bg.wasm",
      "/static/cavea/glyphs.png",
    ],
  ] as const) {
    const origin = serve(
      [
        { prefix: "/page/", dir: page },
        { prefix: "/lib/v0/", dir: DIST },
        { prefix: "/js/", dir: scriptOnly },
        { prefix: "/static/cavea/", dir: DIST },
      ],
      { html: (t) => t.replace("{{script}}", src).replace("{{integrity}}", sri) },
    );
    const v = await (await chromium()).visit(`${origin}/page/${query}`);
    rows.push({
      check,
      tool: "cavea.min.js",
      ok: drew(v) && loadedFrom(v, wasm, atlas),
      detail: `script at ${src}, integrity pinned: ${outcome(v)}; ${assets(v)}`,
    });
  }
}

/**
 * A policy as directives; `tighten` lists sources to remove one at a time,
 * each with what the map should then report.
 */
interface Policy {
  directives: Record<string, string[]>;
  tighten: Array<{
    directive: string;
    source: string;
    expect: (v: Visit) => boolean;
    says: string;
  }>;
}
const header = (directives: Record<string, string[]>): string =>
  Object.entries(directives)
    .map(([d, sources]) => [d, ...sources].join(" "))
    .join("; ");

/** Visits `url` under `policy`, then under each tightened policy. */
async function csp(
  check: string,
  policy: Policy,
  serveWith: (header: string) => string,
  path: string,
  pass: (v: Visit) => string | null,
): Promise<void> {
  const b = await chromium();
  const full = header(policy.directives);
  const v = await b.visit(`${serveWith(full)}${path}`);
  const failure = pass(v);
  rows.push({
    check,
    tool: "Content-Security-Policy",
    ok: failure === null,
    detail: `${full}: ${outcome(v)}${failure === null ? "" : `; ${failure}`}`,
  });
  for (const t of policy.tighten) {
    const directives = {
      ...policy.directives,
      [t.directive]: (policy.directives[t.directive] ?? []).filter((s) => s !== t.source),
    };
    const tight = await b.visit(`${serveWith(header(directives))}${path}`);
    rows.push({
      check: `${check} without ${t.source} in ${t.directive}`,
      tool: "Content-Security-Policy",
      ok: t.expect(tight),
      detail: `expected ${t.says}: ${outcome(tight)}`,
    });
  }
}

const failsWith = (code: string) => (v: Visit) => v.failed === code;
const TRUSTED_TYPES = { "require-trusted-types-for": ["'script'"] };

async function cspCustom(): Promise<void> {
  const page = join(WORK, "csp");
  await rm(page, { recursive: true, force: true });
  await cp(join(FIXTURES, "csp"), page, { recursive: true });
  await cp(chartFile, join(page, "chart.ccf"));
  await csp(
    "csp-custom (ES module, custom adapter)",
    {
      directives: {
        "default-src": ["'none'"],
        "script-src": ["'self'", "'wasm-unsafe-eval'"],
        "connect-src": ["'self'"],
        ...TRUSTED_TYPES,
      },
      tighten: [
        {
          directive: "script-src",
          source: "'wasm-unsafe-eval'",
          expect: failsWith("wasm_unavailable"),
          says: "wasm_unavailable, at once",
        },
        {
          directive: "connect-src",
          source: "'self'",
          expect: (v) => v.failed === "wasm_unavailable" || v.failed === "chart_fetch_failed",
          says: "wasm_unavailable or chart_fetch_failed, whichever fetch fails first, after the retries",
        },
      ],
    },
    (h) =>
      serve(
        [
          { prefix: "/pkg/", dir: DIST },
          { prefix: "/", dir: page },
        ],
        { headers: () => ({ "content-security-policy": h }) },
      ),
    "/custom.html",
    (v) => (drew(v) ? null : "the map did not draw"),
  );
}

async function cspCdn(): Promise<void> {
  const page = join(WORK, "csp-cdn");
  await rm(page, { recursive: true, force: true });
  await cp(join(FIXTURES, "script-tag"), page, { recursive: true });
  await cp(chartFile, join(page, "chart.ccf"));
  const cors = { "access-control-allow-origin": "*" };
  const cdn = serve([{ prefix: "/cavea/0.0.0/", dir: DIST }], { headers: () => cors });
  // The same files with one byte of the WASM changed: the pinned hash must refuse it.
  const tampered = join(WORK, "tampered");
  await rm(tampered, { recursive: true, force: true });
  await cp(DIST, tampered, { recursive: true });
  const wasm = await readFile(join(tampered, "cavea_web_bg.wasm"));
  wasm[wasm.length - 1] = (wasm.at(-1) ?? 0) ^ 1;
  await writeFile(join(tampered, "cavea_web_bg.wasm"), wasm);
  const evil = serve([{ prefix: "/cavea/0.0.0/", dir: tampered }], { headers: () => cors });
  const sri = await integrity(join(DIST, "cavea.min.js"));
  const src = `${cdn}/cavea/0.0.0/cavea.min.js`;
  const html = (t: string): string => t.replace("{{script}}", src).replace("{{integrity}}", sri);
  const pageWith = (h: string): string =>
    serve([{ prefix: "/", dir: page }], {
      host: "localhost",
      headers: () => ({ "content-security-policy": h }),
      html,
    });
  await csp(
    "csp-cdn (script tag from another origin, SRI)",
    {
      directives: {
        "default-src": ["'none'"],
        "script-src": ["'self'", cdn, "'wasm-unsafe-eval'"],
        "connect-src": ["'self'", cdn],
        ...TRUSTED_TYPES,
      },
      tighten: [
        {
          directive: "script-src",
          source: "'wasm-unsafe-eval'",
          expect: failsWith("wasm_unavailable"),
          says: "wasm_unavailable",
        },
        {
          directive: "connect-src",
          source: cdn,
          expect: failsWith("wasm_unavailable"),
          says: "wasm_unavailable after the retries",
        },
      ],
    },
    pageWith,
    "/",
    (v) =>
      drew(v) &&
      loadedFrom(v, `${cdn}/cavea/0.0.0/cavea_web_bg.wasm`, `${cdn}/cavea/0.0.0/glyphs.png`)
        ? null
        : "not drawn from the CDN's files",
  );
  // The tampered WASM, under the same script (its integrity is the real build's).
  const evilScript = `${evil}/cavea/0.0.0/cavea.min.js`;
  const o = serve([{ prefix: "/", dir: page }], {
    host: "localhost",
    html: (t) => t.replace("{{script}}", evilScript).replace("{{integrity}}", sri),
  });
  const v = await (await chromium()).visit(`${o}/`);
  rows.push({
    check: "csp-cdn, WASM changed on the CDN",
    tool: "cavea.min.js integrity",
    ok: v.failed === "wasm_unavailable",
    detail: `expected wasm_unavailable (the fetch's integrity fails): ${outcome(v)}`,
  });
}

/** `cavea-server` on its own database, with a workspace, a published chart, an event and a key. */
async function caveaServer(): Promise<{ base: string; key: string; event: string }> {
  const pg = "postgres://cavea:cavea@127.0.0.1:5432";
  const db = `cavea_compat_${process.pid}`;
  await sh(["docker", "compose", "-f", "deploy/compose.yaml", "up", "-d", "--wait"]);
  await sh(["nix", "develop", "-c", "psql", `${pg}/cavea`, "-c", `CREATE DATABASE ${db}`]);
  cleanup.push(async () => {
    await sh([
      "nix",
      "develop",
      "-c",
      "psql",
      `${pg}/cavea`,
      "-c",
      `DROP DATABASE IF EXISTS ${db} WITH (FORCE)`,
    ]);
  });
  await sh(["cargo", "build", "-q", "-p", "cavea-server"], { env: { CARGO_BUILD_JOBS: "8" } });
  const admin = randomBytes(24).toString("hex");
  const port = freePort();
  const proc = start([join(ROOT, "target/debug/cavea-server")], {
    env: {
      CAVEA_DATABASE_URL: `${pg}/${db}`,
      CAVEA_LISTEN: `127.0.0.1:${port}`,
      CAVEA_ADMIN_KEY: admin,
      CAVEA_LOG_FORMAT: "text",
    },
  });
  const base = `http://127.0.0.1:${port}`;
  for (let i = 0; ; i++) {
    const ok = await fetch(`${base}/readyz`).then(
      (r) => r.ok,
      () => false,
    );
    if (ok) {
      break;
    }
    if (i > 100 || proc.proc.exitCode !== null) {
      throw new Error("cavea-server did not become ready");
    }
    await Bun.sleep(200);
  }
  const call = async (
    path: string,
    key: string,
    body?: unknown,
  ): Promise<Record<string, unknown>> => {
    const res = await fetch(`${base}${path}`, {
      method: "POST",
      headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if (!res.ok) {
      throw new Error(`POST ${path}: HTTP ${res.status} ${await res.text()}`);
    }
    return (await res.json()) as Record<string, unknown>;
  };
  const workspace = await call("/v1/workspaces", admin, { key: "compat", name: "compat" });
  const secret = String((workspace["secret_key"] as { key: string }).key);
  const document = JSON.parse(await readFile(join(WORK, "theatre.json"), "utf8")) as unknown;
  await call("/v1/charts", secret, { key: "theatre", name: "theatre", document });
  await call("/v1/charts/theatre/publish", secret);
  await call("/v1/events", secret, { key: "show", chart: "theatre" });
  const key = String(
    (await call("/v1/keys", secret, { kind: "publishable", name: "compat" }))["key"],
  );
  return { base, key, event: "show" };
}

async function cspServer(): Promise<void> {
  const page = join(WORK, "csp");
  await rm(page, { recursive: true, force: true });
  await cp(join(FIXTURES, "csp"), page, { recursive: true });
  const server = await caveaServer();
  const ws = server.base.replace("http:", "ws:");
  // `auto` (the default) takes the WebSocket; `sse` forced is the smallest policy.
  for (const transport of ["auto", "sse"] as const) {
    const auto = transport === "auto";
    const live = (v: Visit): boolean => (auto ? v.wsFrames > 0 : v.sseMessages > 0);
    const query = `/server.html?server=${encodeURIComponent(server.base)}&key=${server.key}&event=${server.event}&transport=${transport}`;
    await csp(
      `csp-server (caveaServer on another origin, transport ${transport})`,
      {
        directives: {
          "default-src": ["'none'"],
          "script-src": ["'self'", "'wasm-unsafe-eval'"],
          "connect-src": auto ? ["'self'", server.base, ws] : ["'self'", server.base],
          ...TRUSTED_TYPES,
        },
        tighten: [
          {
            directive: "connect-src",
            source: server.base,
            // Whichever comes first: the chart (from the event's info) or the first state.
            expect: (v: Visit) =>
              v.failed === "state_connect_failed" || v.failed === "chart_fetch_failed",
            says: "state_connect_failed or chart_fetch_failed (the event, its chart and its state are refused)",
          },
          ...(auto
            ? [
                {
                  directive: "connect-src",
                  source: ws,
                  // Two refused WebSockets, then SSE (05 §4.1), within the visit's wait.
                  expect: (v: Visit) =>
                    v.firstFrame !== undefined && v.wsFrames === 0 && v.sseMessages > 0,
                  says: "the map drawn, realtime over EventSource after the WebSocket is refused twice",
                },
              ]
            : []),
        ],
      },
      (h) =>
        serve(
          [
            { prefix: "/pkg/", dir: DIST },
            { prefix: "/", dir: page },
          ],
          {
            host: "localhost",
            headers: () => ({ "content-security-policy": h }),
          },
        ),
      query,
      (v) =>
        !drew(v)
          ? "the map did not draw"
          : live(v)
            ? null
            : `no ${auto ? "WebSocket" : "SSE"} frames arrived`,
    );
  }
}

// --------------------------------------------------------------------- main

const CHECKS: Record<string, () => Promise<void>> = {
  "script-tag": scriptTag,
  vite: () => bundler("vite", "vite", "dist", false),
  "vite-dev": viteDev,
  webpack: () => bundler("webpack", "webpack", "dist", true),
  rollup: () => bundler("rollup", "rollup", "dist", true),
  esbuild: () => bundler("esbuild", "esbuild", "dist", true),
  next,
  react18,
  "csp-custom": cspCustom,
  "csp-server": cspServer,
  "csp-cdn": cspCdn,
};

const wanted = process.argv.slice(2);
const unknown = wanted.filter((w) => !(w in CHECKS));
if (unknown.length > 0) {
  console.error(`unknown check ${unknown.join(", ")}; the checks are: ${ALL}`);
  process.exit(2);
}
try {
  await mkdir(WORK, { recursive: true });
  await prepare();
  for (const name of wanted.length > 0 ? wanted : ALL.split(" ")) {
    process.stderr.write(`${name}…\n`);
    try {
      await CHECKS[name]?.();
    } catch (error) {
      rows.push({
        check: name,
        tool: "",
        ok: false,
        detail: String(error).split("\n").slice(0, 8).join(" "),
      });
    }
  }
  if (browser !== undefined) {
    console.log(
      `\n${await browser.version()}, headless, WebGL2 on SwiftShader (software rendering)\n`,
    );
  }
} finally {
  while (cleanup.length > 0) {
    const stop = cleanup.pop() ?? (() => undefined);
    await Promise.resolve(stop()).catch((error: unknown) =>
      console.error(`cleanup: ${String(error)}`),
    );
  }
}
console.log("| Check | Tool | Result | What was checked |\n|---|---|---|---|");
for (const row of rows) {
  console.log(
    `| ${row.check} | ${row.tool} | ${row.ok ? "pass" : "FAIL"} | ${row.detail.replaceAll("|", "\\|")} |`,
  );
}
process.exit(rows.every((r) => r.ok) ? 0 : 1);
