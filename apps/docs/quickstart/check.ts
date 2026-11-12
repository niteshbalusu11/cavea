// The quick start, run as a reader runs it (D-82): every command and file of
// `content/quick-start.md` is taken from the page's source, with the site's
// variables filled in, and run in order in a scratch directory with Docker;
// the console steps the page describes are done in headless Chromium by the
// console's own labels; then the page from step 7 is served and a seat on it
// is clicked, and the server is asked whether that seat is held.
//
//   nix develop -c bun apps/docs/quickstart/check.ts [--image TAG] [--path console|api|both] [--screenshots]
//
// Before the first release the image and the seat map come from the
// repository, as the page "Before the first release" says: without --image
// the server image is built (and removed at the end), and the seat map is
// `packages/renderer/dist` (run `cargo xtask build-web` and `bun run build` in
// `packages/renderer` first), copied beside the page as `cavea/`.
//
// What the reader would type differently, and the check does on purpose:
// `docker compose` runs under a project name of its own and the two ports are
// free ones picked at run time, so that runs never collide with each other or
// with anything else on the machine; the publishable-key placeholder is
// replaced with the key; the API path's secret key comes from the console's
// API rather than its screen. Everything started is removed at the end.
//
// `--path console` (the page's steps 3 to 6 in the console), `api` (the
// "Prefer the API?" block instead), or `both` (default: one after the other,
// each on its own server). `--screenshots` retakes the page's screenshots
// (light theme, WebP) into `static/images/` during the console path.

import { randomBytes } from "node:crypto";
import { cp, mkdir, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { Browser, pinned } from "../../../tools/chromium.ts";
import { Tab, until } from "../../../tools/e2e/tab.ts";
import { catalogue, msg } from "../../console/src/i18n/messages.ts";
import { fences, substitute } from "../src/markdown.ts";

const HERE = join(import.meta.dir, "..");
const ROOT = join(HERE, "../..");
const argv = Bun.argv.slice(2);
const option = (name: string): string | undefined => {
  const at = argv.indexOf(name);
  return at < 0 ? undefined : argv[at + 1];
};
const givenImage = option("--image");
const which = option("--path") ?? "both";
const screenshots = argv.includes("--screenshots");
if (!["console", "api", "both"].includes(which)) throw new Error(`--path ${which}?`);

const hex = (n: number): string => randomBytes(n).toString("hex");
const started = performance.now();
const log = (text: string): void =>
  console.log(`[${((performance.now() - started) / 1000).toFixed(1)} s] ${text}`);
const cleanup: Array<() => Promise<void>> = [];

const IMAGE = givenImage ?? `cavea-server:qs-${hex(4)}`;
const SCRIPT = "cavea/cavea.min.js";
const PLACEHOLDER = "pk_paste-your-publishable-key-here";
const OWNER = { email: "owner@example.com", password: `quick start ${hex(8)}` };
const DIST = join(ROOT, "packages/renderer/dist");
const ASSETS = ["cavea.min.js", "cavea_web_bg.wasm", "glyphs.png"];

function check(ok: unknown, what: string): asserts ok {
  if (!ok) throw new Error(`quick start check failed: ${what}`);
}

function freePort(): number {
  const server = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } });
  const port = server.port;
  server.stop(true);
  return port;
}

async function run(
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
    throw new Error(
      `${cmd.slice(0, 4).join(" ")} … exited with ${code}:\n${`${out}\n${err}`.trim().slice(-3000)}`,
    );
  }
  return out;
}

// ------------------------------------------------------------------ the page

interface Step {
  kind: "shell" | "file";
  qs: string;
  code: string;
  file?: string;
}

/** The page's runnable blocks, in order, with the site's variables filled in. */
async function page(): Promise<{ steps: Step[]; source: string }> {
  const site = (await Bun.file(join(HERE, "site.json")).json()) as { vars: Record<string, string> };
  const raw = await Bun.file(join(HERE, "content/quick-start.md")).text();
  const source = substitute(
    raw,
    { ...site.vars, image: IMAGE, renderer_script: SCRIPT },
    "quick-start.md",
  );
  const steps: Step[] = fences(source)
    .filter((f) => f.meta.qs !== undefined)
    .map((f) => ({
      kind: f.meta.file === undefined ? "shell" : "file",
      qs: f.meta.qs ?? "",
      code: f.code,
      ...(f.meta.file === undefined ? {} : { file: f.meta.file }),
    }));
  const ids = steps.map((s) => s.qs);
  for (const id of ["dir", "up", "link", "api", "down"]) {
    check(ids.includes(id), `the page has a block marked qs=${id}`);
  }
  check(steps.filter((s) => s.kind === "file").length === 2, "the page has two files");
  return { steps, source };
}

/**
 * Every bold phrase in the console steps names something on screen: a
 * label from the console's message catalogue (a template's `{…}` stands for
 * any text), or one of the page's own call-outs.
 */
function labelsAreTheConsoles(source: string): void {
  const own = new Set([
    "setup link",
    "publishable key",
    "Copy the link into your browser.",
    "copy the publishable key",
    "Note",
    "Not yet",
  ]);
  const values = Object.values(catalogue as Record<string, string>);
  const patterns = values.map(
    (v) => new RegExp(`^${v.replace(/[.*+?^$()|[\]\\]/g, "\\$&").replace(/\{\w+\}/g, ".+")}$`),
  );
  const unknown: string[] = [];
  for (const m of source.matchAll(/\*\*([^*]+)\*\*/g)) {
    const text = (m[1] ?? "").trim();
    if (own.has(text) || values.includes(text) || patterns.some((p) => p.test(text))) continue;
    unknown.push(text);
  }
  check(unknown.length === 0, `bold labels the console does not show: ${unknown.join(" | ")}`);
}

// ----------------------------------------------------------------- one run

interface Run {
  project: string;
  server: number;
  web: number;
  dir: string;
  cwd: string;
  base: string;
  browser: Browser;
  publishable: string;
  secret: string;
}

/** The page's text as this run needs it: its own project name and ports. */
function localise(r: Run, text: string): string {
  return text
    .replaceAll("127.0.0.1:8080:8080", `127.0.0.1:${r.server}:8080`)
    .replaceAll("localhost:8080", `localhost:${r.server}`)
    .replaceAll("127.0.0.1:8000:80", `127.0.0.1:${r.web}:80`)
    .replaceAll("localhost:8000", `localhost:${r.web}`)
    .replaceAll("docker compose ", `docker compose -p ${r.project} `);
}

async function shell(r: Run, step: Step, env: Record<string, string> = {}): Promise<string> {
  const marker = join(r.dir, ".cwd");
  const out = await run(
    [
      "bash",
      "-c",
      `set -euo pipefail\ncd "$1"\n${localise(r, step.code)}\npwd > "$2"`,
      "_",
      r.cwd,
      marker,
    ],
    { env },
  );
  r.cwd = (await Bun.file(marker).text()).trim();
  return out;
}

async function file(
  r: Run,
  step: Step,
  replace: (text: string) => string = (t) => t,
): Promise<void> {
  const path = join(r.cwd, step.file ?? "");
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, replace(localise(r, step.code)));
}

/** A fresh tab, light theme. */
async function tab(r: Run, name: string, width = 1280, height = 800): Promise<Tab> {
  const t = await Tab.open(r.browser, name, width, height);
  await t.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-color-scheme", value: "light" }],
  });
  return t;
}

async function shoot(t: Tab, name: string): Promise<void> {
  if (!screenshots) return;
  const { data } = (await t.send("Page.captureScreenshot", { format: "webp", quality: 55 })) as {
    data: string;
  };
  await Bun.write(join(HERE, "static/images", `${name}.webp`), Buffer.from(String(data), "base64"));
  log(`screenshot ${name}.webp`);
}

/** The console's API with a session, as the console itself calls it. */
async function consoleSession(r: Run): Promise<{ headers: Record<string, string> }> {
  const signIn = await fetch(`${r.base}/console/api/session`, {
    method: "POST",
    headers: { origin: r.base, "content-type": "application/json" },
    body: JSON.stringify(OWNER),
  });
  check(signIn.status === 200, `console sign-in ${signIn.status}`);
  const me = (await signIn.json()) as { csrf_token: string; workspaces: Array<{ id: string }> };
  const cookie = (signIn.headers.get("set-cookie") ?? "").split(";")[0] ?? "";
  return {
    headers: {
      origin: r.base,
      cookie,
      "cavea-csrf-token": me.csrf_token,
      "cavea-workspace": me.workspaces[0]?.id ?? "",
    },
  };
}

async function secretKey(r: Run): Promise<string> {
  const { headers } = await consoleSession(r);
  const reply = await fetch(`${r.base}/v1/keys`, {
    method: "POST",
    headers: { ...headers, "content-type": "application/json" },
    body: JSON.stringify({ kind: "secret", name: "Quick start backend" }),
  });
  check(reply.status === 201, `secret key ${reply.status}`);
  return String(((await reply.json()) as { key: string }).key);
}

async function seatStatus(r: Run, label: string): Promise<string> {
  const reply = await fetch(
    `${r.base}/v1/events/opening-night/objects?label=${encodeURIComponent(label)}`,
    { headers: { authorization: `Bearer ${r.secret}` } },
  );
  check(reply.ok, `objects ${reply.status}`);
  const body = (await reply.json()) as { data: Array<{ status: string }> };
  return String(body.data[0]?.status);
}

/** Step 2: the setup link from the page's own command, and the owner account through the console. */
async function owner(r: Run, link: string): Promise<Tab> {
  const t = await tab(r, "console");
  t.allow("GET", /^\/favicon\.ico$/, 404);
  t.expect("GET", /^\/console\/api\/me$/, 401);
  await t.goto(link);
  await t.find({ role: "heading", name: msg("setup.title") }, undefined, 30_000);
  await t.enter({ role: "textbox", name: msg("setup.email") }, OWNER.email);
  await t.enter({ role: "textbox", name: msg("setup.password") }, OWNER.password);
  await t.enter({ role: "textbox", name: msg("setup.confirm") }, OWNER.password);
  await t.click({ role: "button", name: msg("setup.submit") });
  await t.find({ role: "heading", name: msg("charts.title") });
  return t;
}

const DIALOG = "dialog|alertdialog";

/** Steps 3 to 6 in the console, as the page says them. */
async function consoleSteps(r: Run, t: Tab): Promise<void> {
  // 3. Draw a chart.
  await t.click({ role: "button", name: msg("charts.new"), nth: 0 });
  const form = await t.find({ role: DIALOG, name: msg("charts.newTitle") });
  await t.enter({ role: "textbox", name: msg("charts.name") }, "Grand Hall", form);
  const chartKey = await t.value(await t.find({ role: "textbox", name: msg("charts.key") }, form));
  check(chartKey === "grand-hall", `the chart's key fills in as grand-hall, not ${chartKey}`);
  await t.click({ role: "button", name: msg("charts.create") }, form);
  await t.gone({ role: DIALOG });
  await t.click({ role: "link", name: "Grand Hall" });
  await t.click({ role: "link", name: msg("charts.open") });
  const canvas = await t.find(
    { role: "application", name: msg("canvas.label", { name: "Grand Hall" }) },
    undefined,
    30_000,
  );
  await t.click({ role: "button", name: msg("tour.skip") });
  await t.click({ role: "button", name: msg("tool.rowBlock") });
  const [left, top, width, height] = await t.box(canvas);
  await t.drag(
    [left + width * 0.3, top + height * 0.35],
    [left + width * 0.7, top + height * 0.6],
    12,
  );
  const inspector = await t.find({ role: "region", name: msg("panel.inspector") });
  await t.enter({ role: "textbox", name: msg("field.rows") }, "10", inspector);
  await t.key("Enter");
  await t.enter({ role: "textbox", name: msg("field.seats") }, "20", inspector);
  await t.key("Enter");
  const bar = await t.find({ role: "contentinfo" });
  await until("the status bar to read 200 seats", async () =>
    (await t.text(bar)).includes(msg("status.seats", { n: "200" })),
  );
  const banner = await t.find({ role: "banner" });
  await until(
    "the top bar to say Saved",
    async () => (await t.text(banner)).includes(msg("save.saved")),
    30_000,
  );
  if (screenshots) {
    // The page's picture: no notice left over from creating the chart, the block filling the view.
    for (const n of await t.all({ role: "button", name: msg("action.close") })) {
      await t.clickAt(...(await t.centre(n.id)));
    }
    await t.click({ role: "button", name: new RegExp(`^${msg("status.zoomFit")}`) }, bar);
    await Bun.sleep(800);
  }
  await shoot(t, "designer");

  // 4. Publish.
  await t.click({ role: "button", name: msg("topbar.publish") }, banner);
  const dialog = await t.find({ role: DIALOG, name: msg("publish.title", { name: "Grand Hall" }) });
  await t.click({ role: "button", name: msg("publish.action") }, dialog);
  const done = msg("publish.done", { version: 1, seats: "200" });
  await until(
    `the dialog to say “${done}”`,
    async () => (await t.text(dialog)).includes(done),
    30_000,
  );
  await t.click({ role: "button", name: msg("action.close") }, dialog);
  await t.gone({ role: DIALOG });

  // 5. Create an event.
  await t.click({ role: "button", name: msg("menu.label") }, banner);
  await t.click({ role: "menuitem", name: msg("menu.console") });
  const frame = await t.find({ role: "navigation", name: msg("nav.label") });
  await t.click({ role: "link", name: msg("nav.events") }, frame);
  await t.click({ role: "button", name: msg("events.new"), nth: 0 });
  const eventForm = await t.find({ role: DIALOG, name: msg("events.newTitle") });
  await t.enter({ role: "textbox", name: msg("events.name") }, "Opening Night", eventForm);
  const eventKey = await t.value(
    await t.find({ role: "textbox", name: msg("events.key") }, eventForm),
  );
  check(eventKey === "opening-night", `the event's key fills in as opening-night, not ${eventKey}`);
  const select = await t.find({ role: "combobox", name: msg("events.chart") }, eventForm);
  const choice = "Grand Hall (grand-hall)";
  // A native select: chosen as a person's choice would be, with its events.
  await t.call(
    select,
    `function () {
      const o = [...this.options].find((o) => o.text === ${JSON.stringify(choice)});
      if (!o) throw new Error("no option ${choice}");
      this.value = o.value;
      this.dispatchEvent(new Event("input", { bubbles: true }));
      this.dispatchEvent(new Event("change", { bubbles: true }));
    }`,
  );
  await until(
    "version 1 to be the chosen chart version",
    async () =>
      (await t.value(
        await t.find({ role: "combobox", name: msg("events.version") }, eventForm),
      )) === "1",
  );
  await t.click({ role: "button", name: msg("events.create") }, eventForm);
  await t.gone({ role: DIALOG });

  // 6. Create a publishable key.
  await t.click({ role: "link", name: msg("nav.developers") }, frame);
  await t.click({ role: "button", name: msg("keys.new"), nth: 0 });
  const keyForm = await t.find({ role: DIALOG, name: msg("keys.newTitle") });
  await t.enter({ role: "textbox", name: msg("keys.name") }, "Buyer page", keyForm);
  await t.click({ role: "radio", name: new RegExp(`^${msg("keys.publishable")}`) }, keyForm);
  await t.click({ role: "button", name: msg("keys.create") }, keyForm);
  const shown = await t.find({
    role: DIALOG,
    name: msg("keys.shownTitle", { name: "Buyer page" }),
  });
  r.publishable = await t.value(
    await t.find({ role: "textbox", name: msg("keys.publishable") }, shown),
  );
  await t.click({ role: "button", name: msg("action.close") }, shown);
  check(/^pk_[0-9a-f]{64}$/.test(r.publishable), "a publishable key from the console");
  r.secret = await secretKey(r);
}

/** The "Prefer the API?" block instead of steps 3 to 6. */
async function apiSteps(r: Run, steps: Step[]): Promise<void> {
  r.secret = await secretKey(r);
  // "Download the sample chart document theatre.json": the file the site serves.
  await cp(
    join(ROOT, "apps/console/src/designer/templates/theatre.json"),
    join(r.cwd, "theatre.json"),
  );
  let out = "";
  for (const step of steps.filter((s) => s.qs === "api")) {
    out += await shell(r, step, { CAVEA_SECRET_KEY: r.secret });
  }
  const keys = [...out.matchAll(/"key":"(pk_[0-9a-f]{64})"/g)].map((m) => m[1] ?? "");
  check(keys.length === 1, `the last call prints one publishable key (found ${keys.length})`);
  r.publishable = keys[0] ?? "";
  check(
    out.includes('"key":"grand-hall"') && out.includes('"key":"opening-night"'),
    "chart and event made",
  );
}

/** Step 8: the page served, a seat clicked, held on the server, clicked again, released. */
async function holdASeat(r: Run, staff: Tab | null): Promise<string> {
  const buyer = await tab(r, "buyer");
  buyer.allow("GET", /^\/favicon\.ico$/, 404);
  await buyer.goto(`http://localhost:${r.web}/`);
  const map = await buyer.find({ role: "application" }, undefined, 30_000);
  const text = () => buyer.evaluate<string>("document.getElementById('selection').textContent");
  await until("the map to draw", async () => (await text()) === "Click a seat to hold it.", 30_000);
  await Bun.sleep(1000);
  // A seat under the pointer: points near the map's centre, a double tap's
  // window apart so that no two clicks make a zoom.
  const [left, top, width, height] = await buyer.box(map);
  let held = "";
  let at: [number, number] = [0, 0];
  search: for (let ring = 0; ring < 6; ring++) {
    for (let dx = -ring; dx <= ring; dx++) {
      for (let dy = -ring; dy <= ring; dy++) {
        if (Math.max(Math.abs(dx), Math.abs(dy)) !== ring) continue;
        at = [left + width / 2 + dx * 7, top + height * 0.4 + dy * 7];
        await buyer.clickAt(...at);
        await Bun.sleep(400);
        const now = await text();
        if (now.startsWith("Held for you: ")) {
          held = now.slice("Held for you: ".length);
          break search;
        }
      }
    }
  }
  check(held !== "", "a click near the map's centre held a seat");
  await until(
    `${held} to be held on the server`,
    async () => (await seatStatus(r, held)) === "held",
  );
  log(`seat ${held} held, as the server says`);
  // The page's pictures are of the console path's chart.
  if (staff !== null) await shoot(buyer, "buyer-map");

  if (staff !== null) {
    const frame = await staff.find({ role: "navigation", name: msg("nav.label") });
    await staff.click({ role: "link", name: msg("nav.events") }, frame);
    await staff.click({ role: "link", name: "Opening Night" });
    await until(
      "the event manager to count one held seat",
      async () => (await staff.evaluate<string>("document.body.innerText")).includes("1 held"),
      30_000,
    );
    await Bun.sleep(1500);
    await shoot(staff, "event-manager");
  }

  // "It changes back if you click it again in the first window, which releases it."
  await buyer.clickAt(...at);
  await until(
    "the page to say no seat is held",
    async () => (await text()) === "Click a seat to hold it.",
  );
  await until(
    `${held} to be free on the server`,
    async () => (await seatStatus(r, held)) === "free",
  );
  log(`seat ${held} released by clicking it again`);
  return held;
}

async function quickStart(path: "console" | "api", steps: Step[], browser: Browser): Promise<void> {
  const r: Run = {
    project: `cavea-qs-${hex(4)}`,
    server: freePort(),
    web: freePort(),
    dir: join(ROOT, "target/tmp", `quickstart-${hex(4)}`),
    cwd: "",
    base: "",
    browser,
    publishable: "",
    secret: "",
  };
  r.base = `http://localhost:${r.server}`;
  r.cwd = r.dir;
  await mkdir(r.dir, { recursive: true });
  const down = async (): Promise<void> => {
    await run(["docker", "compose", "-p", r.project, "down", "-v", "--remove-orphans"], {
      cwd: r.cwd,
    }).catch(() => undefined);
    await rm(r.dir, { recursive: true, force: true });
  };
  cleanup.push(down);
  log(`${path}: project ${r.project}, server on ${r.server}, page on ${r.web}`);

  const step = (qs: string): Step => {
    const s = steps.find((x) => x.qs === qs);
    check(s !== undefined, `qs=${qs}`);
    return s;
  };
  // 1.
  await shell(r, step("dir"));
  const files = steps.filter((s) => s.kind === "file");
  const compose = files.find((s) => s.file === "compose.yaml");
  check(compose !== undefined, "compose.yaml on the page");
  await file(r, compose);
  const composeText = localise(r, compose.code);
  check(
    composeText.includes(`127.0.0.1:${r.server}:8080`) &&
      composeText.includes(`127.0.0.1:${r.web}:80`),
    "the ports moved",
  );
  await shell(r, step("up"));
  log("compose up: every container healthy");
  // 2.
  const link = (await shell(r, step("link"))).trim();
  check(
    new RegExp(`^http://localhost:${r.server}/console/setup#token=[0-9a-f]{64}$`).test(link),
    `the page's command prints the setup link, not “${link}”`,
  );
  const staff = await owner(r, link);
  log("owner account created from the setup link");
  // 3 to 6.
  if (path === "console") await consoleSteps(r, staff);
  else await apiSteps(r, steps);
  log(`chart published, event created, publishable key ${r.publishable.slice(0, 7)}… made`);
  // 7.
  const html = files.find((s) => s.file === "public/index.html");
  check(html !== undefined, "public/index.html on the page");
  check(html.code.includes(PLACEHOLDER), "the page's placeholder for the key");
  check(html.code.includes(`src="${SCRIPT}"`), "the page loads the seat map from the variable");
  await file(r, html, (t) => t.replace(PLACEHOLDER, r.publishable));
  await mkdir(join(r.cwd, "public/cavea"), { recursive: true });
  for (const a of ASSETS) await cp(join(DIST, a), join(r.cwd, "public/cavea", a));
  // 8.
  const held = await holdASeat(r, path === "console" ? staff : null);
  check(held.length > 0, "a seat was held");
  // 9.
  await shell(r, step("down"));
  const left = await run([
    "docker",
    "ps",
    "-aq",
    "--filter",
    `label=com.docker.compose.project=${r.project}`,
  ]);
  check(left.trim() === "", "docker compose down -v removed every container");
  log(`${path}: passed`);
}

async function main(): Promise<void> {
  const { steps, source } = await page();
  labelsAreTheConsoles(source);
  for (const a of ASSETS) {
    check(
      await Bun.file(join(DIST, a)).exists(),
      `packages/renderer/dist/${a} exists (cargo xtask build-web, then bun run build in packages/renderer)`,
    );
  }
  if (givenImage === undefined) {
    log(`building ${IMAGE}`);
    await run(["docker", "buildx", "build", "-f", "deploy/Dockerfile", "-t", IMAGE, "--load", "."]);
    cleanup.push(async () => {
      await run(["docker", "rmi", IMAGE]).catch(() => undefined);
    });
  }
  const { chromium, fonts } = await pinned(ROOT);
  const browser = await Browser.launch({ chromium, fonts, name: "cavea-quickstart" });
  cleanup.push(() => browser.close());
  const paths: Array<"console" | "api"> =
    which === "both" ? ["console", "api"] : [which as "console" | "api"];
  for (const p of paths) await quickStart(p, steps, browser);
}

try {
  await main();
  console.log(`quick start check passed in ${((performance.now() - started) / 1000).toFixed(0)} s`);
} finally {
  for (const c of cleanup.toReversed()) await c();
}
