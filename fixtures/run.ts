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
// react18, csp-custom, csp-server, csp-cdn, server-sdk; none named runs them
// all (`server-sdk` alone needs no WASM build: it packs `@cavea/server-sdk`,
// type-checks a strict Node project using it and runs it under each Node of
// nixpkgs against `cavea-server`). Fixtures are
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
import { Browser, type Message } from "../tools/chromium.ts";
import { Tab } from "../tools/e2e/tab.ts";

const ROOT = join(import.meta.dir, "..");
const WORK = join(ROOT, "target/tmp/compat");
const FIXTURES = import.meta.dir;
const PKGS = join(WORK, "pkgs");
const DIST = join(ROOT, "packages/renderer/dist");
const ALL =
  "script-tag vite vite-dev webpack rollup esbuild next react18 csp-custom csp-server csp-cdn server-sdk example-page example-react example-next example-node";

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

/**
 * Opens `url` in a fresh context (a cold cache) and waits until the page's
 * `window.compat` has `firstFrame` or `failed`, or `ms` pass.
 */
async function visitWith(browser: Browser, url: string, ms = 30_000): Promise<Visit> {
  const { session, context } = await browser.context();
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
        visit.blocked.push(`${short(urls.get(id) ?? "?")} ${p["blockedReason"] ?? p["errorText"]}`);
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
  const off = browser.on(listener);
  try {
    await browser.send("Network.enable", {}, session);
    await browser.send("Runtime.enable", {}, session);
    await browser.send("Page.enable", {}, session);
    await browser.send("Page.navigate", { url }, session);
    const deadline = Date.now() + ms;
    while (Date.now() < deadline) {
      await Bun.sleep(200);
      const { result } = await browser.send(
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
          // Let a realtime stream deliver its first frames (or fall back:
          // 05 §4.1), then read the errors again: one reported meanwhile fails the check.
          await Bun.sleep(3_000);
          const { result: after } = await browser.send(
            "Runtime.evaluate",
            { expression: "JSON.stringify(window.compat?.errors ?? [])", returnByValue: true },
            session,
          );
          visit.errors = JSON.parse(
            String((after as { value?: string }).value ?? "[]"),
          ) as string[];
          break;
        }
      }
    }
    return visit;
  } finally {
    off();
    await browser.send("Target.disposeBrowserContext", { browserContextId: context });
  }
}

let browser: Browser | undefined;
const chromium = async (): Promise<Browser> => {
  if (browser === undefined) {
    const launched = await Browser.launch({
      chromium: await tool("chromium", "chromium"),
      name: "cavea-compat",
      profile: join(WORK, "profile"),
    });
    cleanup.push(() => launched.close());
    browser = launched;
  }
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

/** Builds and packs the packages `checks` need, and compiles the theatre preset. */
async function prepare(checks: string[]): Promise<void> {
  await mkdir(PKGS, { recursive: true });
  const packages = checks.some((c) => c !== "server-sdk" && c !== "example-node")
    ? ["renderer", "react"]
    : [];
  if (packages.length > 0) {
    try {
      await readFile(join(ROOT, "packages/renderer/wasm/cavea_web_bg.wasm"));
    } catch {
      throw new Error("run `cargo xtask build-web` first");
    }
  }
  if (checks.some((c) => c === "server-sdk" || c === "example-node" || c === "example-next")) {
    packages.push("server-sdk");
  }
  for (const pkg of packages) {
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
async function install(name: string, pkgs = ["renderer", "react"]): Promise<string> {
  const dir = join(WORK, name);
  await mkdir(join(dir, "src"), { recursive: true });
  await cp(join(FIXTURES, name), dir, { recursive: true });
  if (name !== "next" && name !== "react18" && name !== "server-sdk") {
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
  for (const pkg of pkgs) {
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
  const v = await visitWith(await chromium(), `${serve([{ prefix: "/", dir: dist }])}/`);
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
  const v = await visitWith(await chromium(), `http://127.0.0.1:${port}/`);
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
    const v = await visitWith(await chromium(), `${base}${page}`);
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
    const v = await visitWith(await chromium(), `${origin}/page/${query}`);
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
  const v = await visitWith(b, `${serveWith(full)}${path}`);
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
    const tight = await visitWith(b, `${serveWith(header(directives))}${path}`);
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
  const v = await visitWith(await chromium(), `${o}/`);
  rows.push({
    check: "csp-cdn, WASM changed on the CDN",
    tool: "cavea.min.js integrity",
    ok: v.failed === "wasm_unavailable",
    detail: `expected wasm_unavailable (the fetch's integrity fails): ${outcome(v)}`,
  });
}

let servers = 0;

/** `cavea-server` on its own database, with a workspace, a published chart, an event and a key. */
async function caveaServer(): Promise<{
  base: string;
  key: string;
  event: string;
  asset: string;
  admin: string;
  secret: string;
  secretId: string;
}> {
  const pg = "postgres://cavea:cavea@127.0.0.1:5432";
  // One database per call: several checks start a server in one run.
  const db = `cavea_compat_${process.pid}_${++servers}`;
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
  const { key: secret, id: secretId } = workspace["secret_key"] as { key: string; id: string };
  // A background image (04 §7), loaded from the server after the first frame.
  const upload = await fetch(`${base}/v1/assets`, {
    method: "POST",
    headers: { authorization: `Bearer ${secret}`, "content-type": "image/png" },
    body: await readFile(join(ROOT, "crates/cavea-render/golden/background.png")),
  });
  if (!upload.ok) {
    throw new Error(`POST /v1/assets: HTTP ${upload.status} ${await upload.text()}`);
  }
  const asset = String(((await upload.json()) as { hash: string }).hash);
  const document = JSON.parse(await readFile(join(WORK, "theatre.json"), "utf8")) as {
    nextOid: number;
    nodes: unknown[];
  };
  document.nodes.unshift({
    id: document.nextOid++,
    type: "image",
    asset,
    role: "background",
    size: [3000, 1500],
    origin: [0, 1500],
  });
  await call("/v1/charts", secret, { key: "theatre", name: "theatre", document });
  await call("/v1/charts/theatre/publish", secret);
  await call("/v1/events", secret, { key: "show", chart: "theatre" });
  const key = String(
    (await call("/v1/keys", secret, { kind: "publishable", name: "compat" }))["key"],
  );
  return { base, key, event: "show", asset, admin, secret, secretId };
}

/** The packed server SDK in a strict Node project: type-checked with the
 * DOM library off and `skipLibCheck` off, compiled, and run under Node 22
 * and 24 from nixpkgs against `cavea-server` (Node 20 left nixpkgs at its
 * end of life). */
async function serverSdk(): Promise<void> {
  const dir = await install("server-sdk", ["server-sdk"]);
  const server = await caveaServer();
  const tsc = ["node_modules/typescript/bin/tsc", "-p", "."];
  const chart = await readFile(join(WORK, "theatre.json"), "utf8");
  for (const attr of ["nodejs_24", "nodejs_22"]) {
    const out = await sh(["nix", "build", "--no-link", "--print-out-paths", `nixpkgs#${attr}`]);
    const bin = join(out.trim().split("\n").at(-1) ?? "", "bin", "node");
    const version = (await sh([bin, "--version"])).trim();
    const pkg = await versionOf(dir, "typescript");
    try {
      await sh([bin, ...tsc], { cwd: dir });
      const ran = await sh([bin, "out/main.js"], {
        cwd: dir,
        env: { CAVEA_URL: server.base, CAVEA_ADMIN_KEY: server.admin, CAVEA_CHART: chart },
      });
      const seen = JSON.parse(ran.trim().split("\n").at(-1) ?? "{}") as Record<string, unknown>;
      const ok =
        seen["node"] === version &&
        seen["seats"] === seen["free"] &&
        seen["booked"] === 2 &&
        seen["replayed"] === true &&
        seen["conflict"] === "object_not_free" &&
        seen["session"] === 3 &&
        seen["error"] === "CaveaError";
      rows.push({
        check: "server-sdk",
        tool: `Node ${version}, ${pkg}`,
        ok,
        detail: `types resolve under strict, no DOM, skipLibCheck off; ${JSON.stringify(seen)}`,
      });
    } catch (error) {
      rows.push({
        check: "server-sdk",
        tool: `Node ${version}, ${pkg}`,
        ok: false,
        detail: String(error),
      });
    }
  }
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
          : !v.fetched.some((f) => f.url.includes(server.asset) && f.status === 200)
            ? "the background image was not fetched"
            : live(v)
              ? null
              : `no ${auto ? "WebSocket" : "SSE"} frames arrived`,
    );
  }
}

// ----------------------------------------------------------------- examples

// The examples (`examples/`, DOC-009) are written to be copied out and run
// as they are. Each check copies one, points its `@cavea` versions at the
// packed tarballs (the packages are not published yet), installs it with npm
// as its README says, runs it against `cavea-server` and drives it: the pages
// in Chromium, by clicking a seat on the map; the backend over HTTP. Then it
// asks the server what happened to the seats.

const EXAMPLES = join(ROOT, "examples");

/** Example `name` copied to `target/tmp/compat/examples/<name>` and installed. */
async function example(name: string): Promise<{ dir: string; env: Record<string, string> }> {
  const dir = join(WORK, "examples", name);
  await rm(dir, { recursive: true, force: true });
  await cp(join(EXAMPLES, name), dir, {
    recursive: true,
    filter: (src) => !/\/(node_modules|\.next|dist)(\/|$)/.test(src),
  });
  const file = join(dir, "package.json");
  const pkg = JSON.parse(await readFile(file, "utf8")) as Record<string, Record<string, string>>;
  let deps = 0;
  for (const field of ["dependencies", "devDependencies"]) {
    for (const dep of Object.keys(pkg[field] ?? {})) {
      deps++;
      const short = dep.match(/^@cavea\/(.+)$/)?.[1];
      const into = pkg[field];
      if (short !== undefined && into !== undefined) {
        into[dep] = `file:${join(PKGS, `cavea-${short}-0.0.0.tgz`)}`;
      }
    }
  }
  await writeFile(file, JSON.stringify(pkg, null, 2));
  const env = await node(dir);
  if (deps > 0) {
    await sh([join(nodePath, "npm"), "install", "--no-audit", "--no-fund"], { cwd: dir, env });
  }
  return { dir, env };
}

/** Where the event's seat `label` stands on the server. */
async function seatStatus(
  server: { base: string; secret: string; event: string },
  label: string,
): Promise<string> {
  const res = await fetch(
    `${server.base}/v1/events/${server.event}/objects?label=${encodeURIComponent(label)}`,
    { headers: { authorization: `Bearer ${server.secret}` } },
  );
  const body = (await res.json()) as { data: Array<{ status: string }> };
  return body.data[0]?.status ?? "missing";
}

/**
 * Clicks near the middle of the page's map until `held()` names a seat the
 * page lists as chosen; returns it. A double tap's window apart, so no two
 * clicks make a zoom.
 */
async function holdOnMap(t: Tab, held: () => Promise<string | null>): Promise<string> {
  const map = await t.find({ role: "application" }, undefined, 30_000);
  await Bun.sleep(1500);
  const [left, top, width, height] = await t.box(map);
  for (let ring = 0; ring < 8; ring++) {
    for (let dx = -ring; dx <= ring; dx++) {
      for (let dy = -ring; dy <= ring; dy++) {
        if (Math.max(Math.abs(dx), Math.abs(dy)) !== ring) continue;
        await t.clickAt(left + width / 2 + dx * 6, top + height * 0.45 + dy * 6);
        await Bun.sleep(400);
        const label = await held();
        if (label !== null) return label;
      }
    }
  }
  throw new Error("no click near the map's middle chose a seat");
}

/** Waits for `test`, or fails naming `what`. */
async function until<T>(
  what: string,
  test: () => Promise<T | null | undefined>,
  ms = 15_000,
): Promise<T> {
  for (const deadline = Date.now() + ms; Date.now() < deadline; await Bun.sleep(200)) {
    const value = await test();
    if (value !== null && value !== undefined) return value;
  }
  throw new Error(`timed out waiting for ${what}`);
}

/** A page of an example in Chromium: the map, a seat held on it, problems collected. */
async function examplePageTab(url: string): Promise<Tab> {
  const t = await Tab.open(await chromium(), "buyer", 1280, 900);
  cleanup.push(() => t.close());
  t.allow("GET", /favicon/, 404);
  await t.goto(url);
  await t.find({ role: "application" }, undefined, 30_000).catch(async (error: unknown) => {
    await t.collect();
    const seen = [...t.network.slice(-10), ...t.problems.map((p) => p.text.slice(0, 200))];
    throw new Error(`${String(error)}: ${seen.join(" | ")}`, { cause: error });
  });
  return t;
}

/** Whatever the page reported that a person would have seen break. */
async function pageProblems(t: Tab): Promise<string[]> {
  await t.collect();
  return t.problems.map((p) => p.text.slice(0, 200));
}

async function examplePage(): Promise<void> {
  const { dir, env } = await example("plain-page");
  const server = await caveaServer();
  // The README's step 1, and the script from beside the page until the CDN has it.
  const page = join(dir, "index.html");
  const html = (await readFile(page, "utf8"))
    .replace(/const CAVEA_URL = ".*";/, `const CAVEA_URL = "${server.base}";`)
    .replace(/const PUBLISHABLE_KEY = ".*";/, `const PUBLISHABLE_KEY = "${server.key}";`)
    .replace(/const EVENT = ".*";/, `const EVENT = "${server.event}";`)
    .replace(
      /<script src="https:\/\/cdn[^"]*"><\/script>/,
      '<script src="cavea/cavea.min.js"></script>',
    );
  await writeFile(page, html);
  await mkdir(join(dir, "cavea"), { recursive: true });
  for (const f of ["cavea.min.js", "cavea_web_bg.wasm", "glyphs.png"])
    await cp(join(DIST, f), join(dir, "cavea", f));
  const port = freePort();
  const proc = start([join(nodePath, "node"), "serve.js"], {
    cwd: dir,
    env: { ...env, PORT: String(port) },
  });
  await waitFor(proc, /localhost/, 15_000);
  const t = await examplePageTab(`http://localhost:${port}/`);
  t.expect("POST", /^\/api\/checkout$/, 404);
  const text = (id: string) =>
    t.evaluate<string>(`document.getElementById(${JSON.stringify(id)}).textContent`);
  const label = await holdOnMap(t, async () => {
    const first = await t.evaluate<string | null>(
      "document.querySelector('#seats li')?.textContent ?? null",
    );
    return first?.split(" (")[0] ?? null;
  });
  const held = await until(`${label} held on the server`, async () =>
    (await seatStatus(server, label)) === "held" ? true : null,
  );
  const total = await text("total");
  const enabled = await until("Continue enabled", () =>
    t.evaluate<boolean | null>("document.getElementById('continue').disabled ? null : true"),
  );
  await t.click({ role: "button", name: "Continue" });
  const said = await until("the page to say checkout is not available", async () =>
    (await text("problem")) !== "" ? text("problem") : null,
  );
  const posted = t.network.some((n) => /^404 POST .*\/api\/checkout$/.test(n));
  const problems = await pageProblems(t);
  rows.push({
    check: "example-page",
    tool: "examples/plain-page",
    ok: held && /^Total: £\d/.test(total) && enabled && posted && problems.length === 0,
    detail: `${label} held on the server; "${total}"; Continue posted the hold token (${posted ? "seen" : "not seen"}): "${said}"${problems.length > 0 ? `; page problems: ${problems.join(" | ")}` : ""}`,
  });
}

async function exampleReact(): Promise<void> {
  const { dir, env } = await example("react");
  const server = await caveaServer();
  const vite = {
    ...env,
    VITE_CAVEA_URL: server.base,
    VITE_CAVEA_PUBLISHABLE_KEY: server.key,
    VITE_CAVEA_EVENT: server.event,
  };
  await sh(["sh", "-c", await script(dir, "build")], { cwd: dir, env: vite });
  const t = await examplePageTab(
    `${serve([{ prefix: "/", dir: join(dir, "dist") }], { host: "localhost" })}/`,
  );
  const label = await holdOnMap(t, async () => {
    const first = await t.evaluate<string | null>(
      "document.querySelector('section li')?.textContent ?? null",
    );
    return first === null ? null : first;
  });
  const seats = await t.evaluate<string>("document.querySelector('section').textContent");
  const statuses = await until("the seat held on the server", async () => {
    const res = await fetch(`${server.base}/v1/events/${server.event}/objects?status=held`, {
      headers: { authorization: `Bearer ${server.secret}` },
    });
    const body = (await res.json()) as { data: Array<{ label: string }> };
    return body.data.length === 1 ? body.data[0]?.label : null;
  });
  await t.click({ role: "checkbox", name: "Dark map" });
  await Bun.sleep(1000);
  const problems = await pageProblems(t);
  rows.push({
    check: "example-react",
    tool: await versionOf(dir, "vite"),
    ok: /Total: £\d/.test(seats) && problems.length === 0,
    detail: `built (tsc, vite); "${label}" listed, ${statuses} held on the server; dark theme switched${problems.length > 0 ? `; page problems: ${problems.join(" | ")}` : ""}`,
  });
}

async function exampleNext(): Promise<void> {
  const { dir, env } = await example("nextjs");
  const server = await caveaServer();
  const nextEnv = {
    ...env,
    NEXT_TELEMETRY_DISABLED: "1",
    NEXT_PUBLIC_CAVEA_URL: server.base,
    NEXT_PUBLIC_CAVEA_PUBLISHABLE_KEY: server.key,
    NEXT_PUBLIC_CAVEA_EVENT: server.event,
  };
  // The build type-checks and prerenders without the secret key.
  await sh(["next", "build"], { cwd: dir, env: nextEnv });
  const port = freePort();
  const proc = start(["next", "start", "-p", String(port), "-H", "127.0.0.1"], {
    cwd: dir,
    env: { ...nextEnv, CAVEA_SECRET_KEY: server.secret },
  });
  await waitFor(proc, /Ready|started server/i, 60_000);
  const t = await examplePageTab(`http://127.0.0.1:${port}/`);
  const label = await holdOnMap(t, async () => {
    const first = await t.evaluate<string | null>(
      "document.querySelector('aside li')?.textContent ?? null",
    );
    return first?.split(",")[0] ?? null;
  });
  await until(`${label} held on the server`, async () =>
    (await seatStatus(server, label)) === "held" ? true : null,
  );
  await t.click({ role: "button", name: "Checkout" });
  const pay = await t.find({ role: "button", name: /^Pay £[\d.]+ \(fake\)$/ });
  const price = await t.nameOf({ role: "button", name: /^Pay / });
  await t.clickAt(...(await t.centre(pay)));
  await t.find({ role: "heading", name: "Booked" });
  const status = await until(`${label} booked on the server`, async () => {
    const s = await seatStatus(server, label);
    return s === "booked" ? s : null;
  });
  const problems = await pageProblems(t);
  rows.push({
    check: "example-next",
    tool: await versionOf(dir, "next"),
    ok: status === "booked" && problems.length === 0,
    detail: `built and started; ${label} held from the map, checked out ("${price}"), paid and ${status} on the server${problems.length > 0 ? `; page problems: ${problems.join(" | ")}` : ""}`,
  });
}

async function exampleNode(): Promise<void> {
  const { dir, env } = await example("node-backend");
  const server = await caveaServer();
  await sh([join(nodePath, "npm"), "run", "typecheck"], { cwd: dir, env });
  const port = freePort();
  // `npm start` is `node server.ts`: started directly, so that stopping it stops the server.
  const proc = start([join(nodePath, "node"), "server.ts"], {
    cwd: dir,
    env: {
      ...env,
      PORT: String(port),
      CAVEA_URL: server.base,
      CAVEA_EVENT: server.event,
      CAVEA_SECRET_KEY: server.secret,
      CAVEA_SECRET_KEY_ID: server.secretId,
    },
  });
  await waitFor(proc, /listening on/, 15_000);
  const cavea = async (
    method: string,
    path: string,
    body?: unknown,
  ): Promise<Record<string, unknown>> => {
    const res = await fetch(`${server.base}${path}`, {
      method,
      headers: { authorization: `Bearer ${server.secret}`, "content-type": "application/json" },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if (!res.ok) throw new Error(`${method} ${path}: HTTP ${res.status} ${await res.text()}`);
    return (await res.json()) as Record<string, unknown>;
  };
  /** A buyer's browser: its own cookie. */
  const buyer = () => {
    let cookie = "";
    return async (
      path: string,
      body: unknown = {},
    ): Promise<{ status: number; json: Record<string, unknown> }> => {
      const res = await fetch(`http://127.0.0.1:${port}${path}`, {
        method: "POST",
        headers: { "content-type": "application/json", cookie },
        body: JSON.stringify(body),
      });
      cookie = res.headers.get("set-cookie")?.split(";")[0] ?? cookie;
      return { status: res.status, json: (await res.json()) as Record<string, unknown> };
    };
  };
  const free = (await cavea("GET", `/v1/events/${server.event}/objects?status=free&limit=8`)) as {
    data: Array<{ label: string; category: { key: string } }>;
  };
  const seats = free.data;
  const PRICES: Record<string, number> = {
    premium: 6500,
    standard: 4500,
    value: 2500,
    access: 4500,
  };
  const seen: string[] = [];
  const expect = (ok: boolean, what: string): void => {
    if (!ok) throw new Error(what);
    seen.push(what);
  };
  // Hold, price, pay, book; pay again.
  const a = buyer();
  const token = String((await a("/api/hold-token")).json["holdToken"]);
  expect(
    String((await a("/api/hold-token")).json["holdToken"]) === token,
    "one hold token per session",
  );
  const two = seats.slice(0, 2);
  await cavea("POST", `/v1/events/${server.event}/hold`, {
    hold_token: token,
    objects: two.map((s) => s.label),
  });
  const order = await a("/api/checkout");
  const total = two.reduce((sum, s) => sum + (PRICES[s.category.key] ?? 0), 0);
  expect(
    order.status === 200 && order.json["total"] === total,
    `checkout priced ${total} from the categories`,
  );
  const id = String(order.json["orderId"]);
  // A double click on Pay: two requests at once, one payment, one booking.
  const [paid, again] = await Promise.all([a(`/api/orders/${id}/pay`), a(`/api/orders/${id}/pay`)]);
  expect(
    again.json["status"] === "booked" && (proc.output.match(/FAKE payment/g) ?? []).length === 1,
    "two Pay requests at once take one payment",
  );
  const statuses = await Promise.all(two.map((s) => seatStatus(server, s.label)));
  expect(
    paid.json["status"] === "booked" && statuses.every((s) => s === "booked"),
    "paid and booked on the server",
  );
  expect(
    (await a(`/api/orders/${id}/pay`)).json["status"] === "booked",
    "paying again answers booked",
  );
  expect(
    (await buyer()(`/api/orders/${id}/pay`)).status === 404,
    "another buyer cannot pay the order",
  );
  // The hold lost during the payment: refunded, nothing booked.
  const b = buyer();
  const lost = seats[2]?.label ?? "";
  const tokenB = String((await b("/api/hold-token")).json["holdToken"]);
  await cavea("POST", `/v1/events/${server.event}/hold`, { hold_token: tokenB, objects: [lost] });
  const orderB = String((await b("/api/checkout")).json["orderId"]);
  await cavea("POST", `/v1/events/${server.event}/release`, {
    hold_token: tokenB,
    objects: [lost],
  });
  const [refused] = await Promise.all([
    b(`/api/orders/${orderB}/pay`),
    b(`/api/orders/${orderB}/pay`),
  ]);
  expect(
    refused.status === 409 &&
      refused.json["error"] === "hold_expired" &&
      (await seatStatus(server, lost)) === "free" &&
      (proc.output.match(/FAKE refund/g) ?? []).length === 1,
    "a hold lost during payment is refunded once (two Pay requests at once), not booked",
  );
  // Cancelled: released at once.
  const c = buyer();
  const kept = seats[3]?.label ?? "";
  const tokenC = String((await c("/api/hold-token")).json["holdToken"]);
  await cavea("POST", `/v1/events/${server.event}/hold`, { hold_token: tokenC, objects: [kept] });
  const orderC = String((await c("/api/checkout")).json["orderId"]);
  const cancelled = await c(`/api/orders/${orderC}/cancel`);
  expect(
    cancelled.json["status"] === "cancelled" && (await seatStatus(server, kept)) === "free",
    "a cancelled order's seats are free",
  );
  // An expired (here: unknown) hold token at checkout.
  const gone = await buyer()("/api/checkout", { holdToken: "an-expired-hold-token" });
  expect(
    gone.status === 409 && gone.json["error"] === "hold_expired",
    "an ended hold at checkout is hold_expired",
  );
  // A session token the server takes for a page's own hold token.
  const session = String((await buyer()("/api/seat-session")).json["token"]);
  const created = await fetch(`${server.base}/v1/public/hold-tokens`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${server.key}`,
      "content-type": "application/json",
      "cavea-session": session,
    },
    body: JSON.stringify({ event: server.event }),
  });
  expect(created.status === 201 || created.status === 200, "the signed session token is accepted");
  const leaked = /sk_|hold_token":|"hold/.test(proc.output) ? "a key or token in the output" : null;
  rows.push({
    check: "example-node",
    tool: `Node ${(await sh([join(nodePath, "node"), "--version"])).trim()}`,
    ok: leaked === null,
    detail: `${seen.join("; ")}${leaked === null ? "" : `; ${leaked}`}`,
  });
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
  "server-sdk": serverSdk,
  "example-page": examplePage,
  "example-react": exampleReact,
  "example-next": exampleNext,
  "example-node": exampleNode,
};

// `examples` names the four example checks.
const wanted = process.argv
  .slice(2)
  .flatMap((w) =>
    w === "examples" ? ["example-page", "example-react", "example-next", "example-node"] : [w],
  );
const unknown = wanted.filter((w) => !(w in CHECKS));
if (unknown.length > 0) {
  console.error(`unknown check ${unknown.join(", ")}; the checks are: ${ALL}`);
  process.exit(2);
}
try {
  await mkdir(WORK, { recursive: true });
  await prepare(wanted.length > 0 ? wanted : ALL.split(" "));
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
    console.log(`\n${browser.version}, headless, WebGL2 on SwiftShader (software rendering)\n`);
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
