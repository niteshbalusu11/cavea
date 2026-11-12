// The designer guide (`content/designer/`), done as a venue's staff member
// does it: every task of the guide in the real designer, in headless
// Chromium, by the labels the guide prints in bold, against `cavea-server`
// with the console compiled in, on a database of its own.
//
//   cargo xtask build-server
//   nix develop -c bun apps/docs/designer/check.ts [--server <binary>] [--screenshots] [--keep]
//
// `--screenshots` retakes the guide's pictures (`static/images/designer-*.webp`,
// light theme, cropped to what each shows). `--keep` leaves the server up
// at the end with the owner's sign-in printed, to look around by hand.
// Every bold phrase on the guide's pages is checked to be a label the
// console shows (or one of the few call-outs listed below).
//
// Everything started is stopped at the end: the server by its process id,
// the browser, the database (dropped).
import { SQL } from "bun";
import { randomBytes } from "node:crypto";
import { mkdir, readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { Browser, pinned } from "../../../tools/chromium.ts";
import { freePort, Server } from "../../../tools/e2e/server.ts";
import { Tab, until } from "../../../tools/e2e/tab.ts";
import { msg } from "../../console/src/i18n/messages.ts";
import { boldLabels } from "../src/labels.ts";
import { KNOWN, TASKS, type Guide } from "./tasks.ts";

const HERE = join(import.meta.dir, "..");
const ROOT = join(HERE, "../..");
const PG = process.env["CAVEA_E2E_POSTGRES"] ?? "postgres://cavea:cavea@127.0.0.1:5432";
const argv = Bun.argv.slice(2);
const option = (name: string): string | undefined => {
  const at = argv.indexOf(name);
  return at < 0 ? undefined : argv[at + 1];
};
const BINARY = option("--server") ?? join(ROOT, "target/release/cavea-server");
const screenshots = argv.includes("--screenshots");
const keep = argv.includes("--keep");
const explore = option("--explore");

const hex = (n: number): string => randomBytes(n).toString("hex");
const started = performance.now();
const log = (text: string): void =>
  console.log(`[${((performance.now() - started) / 1000).toFixed(1)} s] ${text}`);
const cleanup: Array<() => Promise<void>> = [];
const OWNER = { email: "box-office@example.org", password: `designer guide ${hex(8)}` };
const DIALOG = "dialog|alertdialog";
const OUT = join(ROOT, "target/tmp/designer-guide");

function check(ok: unknown, what: string): asserts ok {
  if (!ok) throw new Error(`designer guide check failed: ${what}`);
}

// ------------------------------------------------------------ pictures

/** A picture for the guide: the page, or the part of it `clip` names (CSS px). */
async function shoot(
  t: Tab,
  name: string,
  clip?: { x: number; y: number; width: number; height: number },
): Promise<void> {
  if (!screenshots && explore === undefined) return;
  const params: Record<string, unknown> = { format: "webp", quality: 60 };
  if (clip !== undefined) params["clip"] = { ...clip, scale: 1 };
  const { data } = (await t.send("Page.captureScreenshot", params)) as { data: string };
  const bytes = Buffer.from(String(data), "base64");
  const to = screenshots
    ? join(HERE, "static/images", `designer-${name}.webp`)
    : join(OUT, `designer-${name}.webp`);
  await Bun.write(to, bytes);
  log(`picture ${name}: ${(bytes.byteLength / 1024).toFixed(0)} KiB`);
}

/** The box of an element, grown by `pad` px each side, kept within the page. */
async function around(
  t: Tab,
  ids: number[],
  pad = 8,
): Promise<{ x: number; y: number; width: number; height: number }> {
  let [l, tp, r, b] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const id of ids) {
    const [x, y, w, h] = await t.box(id);
    l = Math.min(l, x);
    tp = Math.min(tp, y);
    r = Math.max(r, x + w);
    b = Math.max(b, y + h);
  }
  const x = Math.max(0, l - pad);
  const y = Math.max(0, tp - pad);
  return { x, y, width: r + pad - x, height: b + pad - y };
}

/** The accessibility outline of the page, while exploring. */
async function look(t: Tab, name: string): Promise<void> {
  if (explore === undefined) return;
  await Bun.write(join(OUT, `${name}.tree.txt`), await t.outline());
  await shoot(t, name);
}

// ---------------------------------------------------------------- main

async function main(): Promise<void> {
  // The guide's bold words are the console's.
  const pages = (await readdir(join(HERE, "content/designer"))).filter((f) => f.endsWith(".md"));
  const unknown: string[] = [];
  for (const p of pages) {
    const text = await Bun.file(join(HERE, "content/designer", p)).text();
    unknown.push(...boldLabels(text, OWN).map((l) => `${p}: ${l}`));
  }
  check(unknown.length === 0, `bold words the console does not show:\n  ${unknown.join("\n  ")}`);

  await mkdir(OUT, { recursive: true });
  const database = `cavea_guide_${process.pid}_${hex(3)}`;
  const admin = new SQL(`${PG}/cavea`, { max: 1 });
  await admin.unsafe(`CREATE DATABASE ${database}`);
  cleanup.push(async () => {
    await admin.unsafe(`DROP DATABASE IF EXISTS ${database} WITH (FORCE)`);
    await admin.close();
  });
  const port = freePort();
  const base = `http://localhost:${port}`;
  const dir = join(OUT, "server");
  await mkdir(join(dir, "tmp"), { recursive: true });
  const server = new Server(
    BINARY,
    {
      CAVEA_DATABASE_URL: `${PG}/${database}`,
      CAVEA_LISTEN: `127.0.0.1:${port}`,
      CAVEA_PUBLIC_URL: base,
      CAVEA_LOG_FORMAT: "text",
      TMPDIR: join(dir, "tmp"),
    },
    base,
    dir,
  );
  cleanup.push(async () => void (await server.stop()));
  await server.start();
  const link = server.log.match(/setup_link="?([^"\s]+)/)?.[1];
  check(link !== undefined, "the server logs a setup link");

  const { chromium, fonts } = await pinned(ROOT);
  const browser = await Browser.launch({
    chromium,
    fonts,
    name: "cavea-guide",
    args: ["--host-resolver-rules=MAP localhost 127.0.0.1"],
  });
  cleanup.push(() => browser.close());
  log(`server ${base}, ${browser.version}`);

  const t = await Tab.open(browser, "staff", 1440, 900);
  await t.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-color-scheme", value: "light" }],
  });
  t.allow("GET", /^\/favicon\.ico$/, 404);
  t.expect("GET", /^\/console\/api\/me$/, 401);
  await t.goto(link);
  await t.find({ role: "heading", name: msg("setup.title") }, undefined, 30_000);
  await t.enter({ role: "textbox", name: msg("setup.email") }, OWNER.email);
  await t.enter({ role: "textbox", name: msg("setup.password") }, OWNER.password);
  await t.enter({ role: "textbox", name: msg("setup.confirm") }, OWNER.password);
  await t.click({ role: "button", name: msg("setup.submit") });
  await t.find({ role: "heading", name: msg("charts.title") });

  const g: Guide = { t, browser, base, out: OUT, shoot, look, log, owner: OWNER };
  for (const task of TASKS) {
    log(task.name);
    await task.run(g);
    // Whatever the page reported meanwhile fails the check: a person would have seen it break.
    await t.collect();
    const problems = t.problems.splice(0).filter((p) => p.kind !== "request");
    check(problems.length === 0, `the page reported, during “${task.name}”:\n  ${problems.map((p) => p.text.slice(0, 300)).join("\n  ")}`);
  }
  for (const k of KNOWN) log(`known product bug, worked around as the guide says: ${k}`);
  if (keep) {
    console.log(`\n--keep: ${base}/console/charts, ${OWNER.email} / ${OWNER.password}. Ctrl+C stops.`);
    await new Promise(() => undefined);
  }
}

/** Bold phrases on the guide's pages that are not console labels: its own call-outs. */
const OWN = ["Note", "Not yet", "Warning"];


try {
  await main();
  console.log(`designer guide check passed in ${((performance.now() - started) / 1000).toFixed(0)} s`);
} finally {
  for (const c of cleanup.toReversed()) await c().catch((e: unknown) => console.error(String(e)));
  void until;
  void rm;
}
