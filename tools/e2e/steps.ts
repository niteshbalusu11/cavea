// The steps of the end-to-end check, in order; each ends in checks a machine
// evaluates. The words on screen come from the console's own catalogue
// (`msg`), so a changed string changes what the check looks for with it.
import { createHmac, randomBytes } from "node:crypto";
import type { Browser } from "../chromium.ts";
import { msg } from "../../apps/console/src/i18n/messages.ts";
import type { Server } from "./server.ts";
import { Tab, until } from "./tab.ts";

/** What the steps share. */
export interface Run {
  root: string;
  out: string;
  /** The console's and the API's origin (`http://localhost:<port>`). */
  base: string;
  /** The buyer page's origin, another one. */
  buyer: string;
  browser: Browser;
  server: Server;
  /** Open pages by name; every one is checked for problems after each step. */
  tabs: Map<string, Tab>;
  /** Values that must never reach the server's log, by what they are. */
  secrets: Map<string, string>;
  /** Lines for the summary (measurements). */
  notes: string[];
  latency: number[];
}

export interface Step {
  name: string;
  run: (r: Run) => Promise<void>;
  /**
   * Set while the step fails because of a product bug that is written up but
   * not fixed: where the write-up is. The run reports it as a known failure
   * and goes on; once the step passes, the mark must go.
   */
  known?: string;
}

/** The numbering panel's way back to the inspector. */
const BACK = msg("panel.back");

/** A modal dialog, whichever of the two roles the browser gives it. */
const DIALOG = "dialog|alertdialog";

const hex = (bytes: number): string => randomBytes(bytes).toString("hex");

/** An authenticator app's code (RFC 6238: SHA-1, six digits) for a
 * base32 key at 30-second step `step`. */
export function totp(key: string, step: number): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of key.replace(/[\s=]/g, "").toUpperCase()) {
    bits += alphabet.indexOf(c).toString(2).padStart(5, "0");
  }
  const bytes = Buffer.from((bits.match(/.{8}/g) ?? []).map((b) => parseInt(b, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const mac = createHmac("sha1", bytes).update(counter).digest();
  const at = (mac[mac.length - 1] ?? 0) & 0xf;
  return String((mac.readUInt32BE(at) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

/** Fails with "expected …, found …" unless `ok`. */
function check(ok: boolean, expected: string, found: unknown): void {
  if (!ok) {
    throw new Error(
      `expected ${expected}; found ${typeof found === "string" ? found : JSON.stringify(found)}`,
    );
  }
}

/** A new page in a context of its own. */
async function tab(r: Run, name: string): Promise<Tab> {
  const t = await Tab.open(r.browser, name);
  r.tabs.set(name, t);
  return t;
}

const page = (r: Run, name: string): Tab => {
  const t = r.tabs.get(name);
  if (t === undefined) throw new Error(`no page ${name}: an earlier step did not open it`);
  return t;
};

/** The owner's account, made in step 1. */
const owner = { email: `owner-${hex(4)}@example.org`, password: hex(16) };
/** The chart drawn in step 2. */
const chart = { name: "Grand Hall", key: "grand-hall" };
/** The event made in step 4. */
const event = { name: "Opening Night", key: "opening-night" };
/** The seat buyer 1 holds and the backend books in step 6, and its hold token. */
const seat = { label: "", hold: "" };
/** Clicks measured for the two-browser latency (each also measures a release, but the last). */
const LATENCY_RUNS = 10;

/**
 * A click on the buyer's map, a person's pace apart: a second press within
 * the map's double-tap window of the last release, on the same spot, is a
 * double tap (a zoom), not a second tap. The window is read from the
 * engine's tuning, so the wait follows it.
 */
let lastTap = 0;
async function tap(r: Run, t: Tab, x: number, y: number): Promise<void> {
  const tuning = await Bun.file(`${r.root}/crates/cavea-render/src/tuning.rs`).text();
  const window = Number(tuning.match(/DOUBLE_TAP_MS: f64 = ([\d.]+)/)?.[1] ?? Number.NaN);
  check(Number.isFinite(window), "DOUBLE_TAP_MS in cavea-render's tuning.rs", "none");
  const wait = lastTap + window + 50 - performance.now();
  if (wait > 0) await Bun.sleep(wait);
  await t.clickAt(x, y);
  lastTap = performance.now();
}

/** Whether the page has a WebSocket to the realtime route that was accepted (101), sends frames and is open. */
function liveSocket(t: Tab): boolean {
  return [...t.sockets.values()].some(
    (w) => w.url.includes("/v1/public/rt") && w.status === 101 && w.frames > 0 && !w.closed,
  );
}

/** A label's row and seat number: `-B-105` is row B, seat 105 (no section). */
function place(label: string): [string, string] {
  const [, row = "", number = ""] = label.match(/^-([A-Z]+)-(\d+)$/) ?? [];
  check(row !== "", "a label of the form -ROW-NUMBER", label);
  return [row, number];
}

/** Whether the element has the keyboard focus, as the accessibility tree says. */
async function focused(t: Tab, id: number): Promise<boolean> {
  const { nodes } = (await t.send("Accessibility.getPartialAXTree", {
    backendNodeId: id,
    fetchRelatives: false,
  })) as { nodes: Array<{ properties?: Array<{ name: string; value: { value?: unknown } }> }> };
  return nodes[0]?.properties?.some((p) => p.name === "focused" && p.value.value === true) ?? false;
}

/**
 * Moves the map's keyboard focus to row `row`, seat `number` with the arrow
 * keys, reading where it is from the map's screen-reader proxy (the
 * active descendant, "Row B, Seat 105. …"), as a screen-reader user hears it.
 */
async function focusSeat(t: Tab, canvas: number, row: string, number: string): Promise<void> {
  if (!(await focused(t, canvas))) {
    // The keyboard's way in: from the page's heading, Tab to the map.
    const heading = await t.find({ role: "heading", name: event.name });
    await t.clickAt(...(await t.centre(heading)));
    for (let presses = 0; !(await focused(t, canvas)); presses++) {
      check(presses < 20, "the map within 20 presses of Tab from the heading", presses);
      await t.key("Tab");
    }
  }
  const where = async (): Promise<[string, number] | null> => {
    const proxy = (await t.all({ role: "button" }, canvas))[0];
    const m = proxy?.name.match(/Row (\w+), Seat (\d+)/);
    return m === null || m === undefined ? null : [m[1] ?? "", Number(m[2])];
  };
  for (let presses = 0; presses < 60; presses++) {
    const at = await where();
    if (at !== null && at[0] === row && at[1] === Number(number)) return;
    const key =
      at === null
        ? "ArrowRight"
        : at[0] < row
          ? "ArrowDown"
          : at[0] > row
            ? "ArrowUp"
            : at[1] < Number(number)
              ? "ArrowRight"
              : "ArrowLeft";
    const before = JSON.stringify(at);
    await t.key(key);
    // Each press moves the focus (or, the first, shows it): wait for the proxy to say so.
    await until(
      `the map's focus to move from ${before} with ${key}`,
      async () => (JSON.stringify(await where()) !== before ? true : undefined),
      5_000,
    );
  }
  throw new Error(`the arrow keys did not reach row ${row}, seat ${number} in 60 presses`);
}

/** A signed-in page's session cookie and CSRF token, among the values the log must not hold. */
async function keepSession(r: Run, t: Tab, who: string): Promise<void> {
  const { cookies } = (await t.send("Network.getCookies")) as {
    cookies: Array<{ name: string; value: string }>;
  };
  check(cookies.length > 0, `a session cookie for ${who}`, "none");
  for (const c of cookies) r.secrets.set(`${who} cookie ${c.name}`, c.value);
  const csrf = await t.evaluate<string>(
    "fetch('/console/api/me').then((r) => r.json()).then((me) => me.csrf_token)",
  );
  r.secrets.set(`${who} CSRF token`, csrf);
}

/** A section of the console's frame, by its link in the frame's navigation. */
async function nav(
  t: Tab,
  section: "nav.events" | "nav.developers" | "nav.settings",
): Promise<void> {
  const frame = await t.find({ role: "navigation", name: msg("nav.label") });
  await t.click({ role: "link", name: msg(section) }, frame);
}

/** The status of one seat, from the server. */
async function status(r: Run, label: string): Promise<string> {
  const found = await api<{ data: Array<{ status: string }> }>(
    r,
    "GET",
    `/v1/events/${event.key}/objects?label=${encodeURIComponent(label)}`,
  );
  return String(found.data[0]?.status);
}

/** Minimum, median, 90th percentile and maximum, in ms. */
function spread(values: number[]): string {
  const v = values.toSorted((a, b) => a - b);
  const at = (q: number): number => v[Math.min(v.length - 1, Math.floor(q * v.length))] ?? NaN;
  return `n ${v.length}, min ${v[0]?.toFixed(0)}, median ${at(0.5).toFixed(0)}, p90 ${at(0.9).toFixed(0)}, max ${v.at(-1)?.toFixed(0)} ms`;
}

/**
 * Where seat `label` is on the buyer's page, in CSS px: the map is asked to
 * centre it (`zoomTo`, the SDK's own call), the pointer moves to the map's
 * centre, and the map's hover report names the seat under it and its
 * centre, which is where the click goes.
 */
async function seatOnScreen(t: Tab, label: string): Promise<[number, number]> {
  await t.evaluate(`window.buyer.map.zoomTo(${JSON.stringify(label)}, { animate: false })`);
  const [left, top, width, height] = await t.box(await t.find({ role: "application" }));
  const centre: [number, number] = [left + width / 2, top + height / 2];
  const hovered = await until(`the map to report ${label} under the pointer`, async () => {
    // Moved off and back, so each look is a fresh hover.
    await t.mouse("mouseMoved", centre[0] + 1, centre[1]);
    await t.mouse("mouseMoved", centre[0], centre[1]);
    const h = await t.evaluate<{ label: string; at: [number, number] } | null>(
      "window.buyer.hovered",
    );
    return h?.label === label ? h : undefined;
  });
  return [left + hovered.at[0], top + hovered.at[1]];
}

/** The member invited in step 8. */
const viewer = { email: `viewer-${hex(4)}@example.org`, password: hex(16) };

/** The keys made in step 5. */
const keys = { secret: "", publishable: "" };

/** A call of the server's API as the customer's backend makes it, with the secret key. */
async function api<T>(r: Run, method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`${r.base}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${keys.secret}`,
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const text = await response.text();
  check(response.ok, `${method} ${path} to succeed`, `${response.status} ${text.slice(0, 300)}`);
  return (text === "" ? null : JSON.parse(text)) as T;
}

/** The parts of a chart document the check reads. */
interface Draft {
  nodes: Array<{
    type: string;
    rows?: Array<{ seats: number }>;
    curve?: unknown;
    category?: string;
    seatLabels?: { start?: number };
  }>;
  categories: Array<{ key: string; label: string }>;
}

/**
 * A GET of the server's API from a console page, with its session (the
 * cookie) and its workspace: what the page itself could read.
 */
async function consoleApi<T>(t: Tab, path: string): Promise<T> {
  const answer = await t.evaluate<{ status: number; body: string }>(`(async () => {
    const me = await (await fetch("/console/api/me")).json();
    const r = await fetch(${JSON.stringify(path)}, { headers: { "cavea-workspace": me.workspaces[0].id } });
    return { status: r.status, body: await r.text() };
  })()`);
  check(
    answer.status === 200,
    `GET ${path} to answer 200`,
    `${answer.status} ${answer.body.slice(0, 300)}`,
  );
  return JSON.parse(answer.body) as T;
}

export const STEPS: Step[] = [
  {
    name: "First run: setup link, owner account, signed-in console",
    async run(r) {
      r.secrets.set("owner password", owner.password);
      r.secrets.set("owner email", owner.email);
      const link = await until(
        "the setup link in the server's log",
        async () => r.server.log.match(/setup_link"?[:=]"?([^"\s]+)/)?.[1],
      );
      check(link.startsWith(`${r.base}/console/setup#token=`), `a setup link on ${r.base}`, link);
      const t = await tab(r, "owner");
      // The console asks who is signed in before anyone is: that answer is 401.
      t.expect("GET", /^\/console\/api\/me$/, 401);
      await t.goto(link);
      await t.find({ role: "heading", name: msg("setup.title") });
      await t.enter({ role: "textbox", name: msg("setup.email") }, owner.email);
      await t.enter({ role: "textbox", name: msg("setup.password") }, owner.password);
      await t.enter({ role: "textbox", name: msg("setup.confirm") }, owner.password);
      await t.click({ role: "button", name: msg("setup.submit") });
      await t.find({ role: "button", name: msg("user.menu", { email: owner.email }) });
      await t.find({ role: "heading", name: msg("charts.title") });
      const at = await t.evaluate<string>("location.pathname");
      check(at.endsWith("/charts"), "the charts list", at);
      await keepSession(r, t, "owner");
      check(
        !(await t.evaluate<string>("location.href")).includes("token"),
        "the token gone from the address bar",
        await t.evaluate<string>("location.href"),
      );
    },
  },
  {
    name: "Draw: a row block drawn, curved, numbered, given a category; no errors; saved",
    async run(r) {
      const t = page(r, "owner");
      // The list's own button; the empty list offers a second one.
      await t.click({ role: "button", name: msg("charts.new"), nth: 0 });
      const form = await t.find({ role: DIALOG, name: msg("charts.newTitle") });
      await t.enter({ role: "textbox", name: msg("charts.name") }, chart.name, form);
      const key = await t.value(await t.find({ role: "textbox", name: msg("charts.key") }, form));
      check(key === chart.key, `the key made from the name, ${chart.key}`, key);
      await t.click({ role: "button", name: msg("charts.create") }, form);
      await t.gone({ role: DIALOG });
      await t.click({ role: "button", name: msg("charts.menu", { name: chart.name }) });
      await t.click({ role: "menuitem", name: msg("charts.open") });
      const canvas = await t.find(
        { role: "application", name: msg("canvas.label", { name: chart.name }) },
        undefined,
        30_000,
      );
      // The first-run tour offers to walk through this; the check does it alone.
      await t.click({ role: "button", name: msg("tour.skip") });
      // An empty chart offers its starting points (07 §16); this path draws
      // by hand, and choosing a drawing tool puts them away.
      const start = await t.find({ role: "region", name: msg("start.title") });
      for (const id of ["theatre", "arena", "banquet"] as const) {
        await t.find({ role: "button", name: new RegExp(`^${msg(`template.${id}`)} `) }, start);
      }
      const bar = await t.find({ role: "contentinfo" });
      // Draw: the row block tool, a press on the canvas and a drag. The
      // readout counts rows and seats live; typing `4x10` and Enter sets them
      // exactly (07 §4, numeric entry), so grid snapping cannot change them.
      await t.click({ role: "button", name: msg("tool.rowBlock") });
      await t.gone({ role: "region", name: msg("start.title") });
      const [left, top, width, height] = await t.box(canvas);
      const [cx, cy] = [left + width / 2, top + height / 2];
      await t.mouse("mouseMoved", cx - 80, cy - 50);
      await t.mouse("mousePressed", cx - 80, cy - 50);
      for (let i = 1; i <= 8; i++) {
        await t.mouse("mouseMoved", cx - 80 + i * 20, cy - 50 + i * 12, 1);
      }
      const live = await until(
        "the live readout of rows and seats",
        async () => (await t.text(bar)).match(/\d+ rows × \d+ seats/)?.[0],
      );
      await t.type("4x10");
      await until("the typed entry in the readout", async () =>
        (await t.text(bar)).includes(msg("readout.entry", { entry: "4x10" })),
      );
      await t.key("Enter");
      await t.mouse("mouseReleased", cx + 80, cy + 46);
      await until("40 seats in the status bar", async () =>
        (await t.text(bar)).includes(msg("status.seats", { n: 40 })),
      );
      r.notes.push(`draw: the drag read “${live}” before 4x10 was typed`);

      // Curve: the inspector's curve field (the tour's other way to it),
      // an arc, its radius typed.
      const inspector = await t.find({ role: "region", name: msg("panel.inspector") });
      await t.click({ role: "button", name: new RegExp(`^${msg("field.curve")} `) }, inspector);
      await t.click({ role: "option", name: msg("curve.arc") });
      await t.enter({ role: "textbox", name: msg("field.curveRadius") }, "1500", inspector);
      await t.key("Enter");
      await t.find(
        { role: "button", name: `${msg("field.curve")} ${msg("curve.arc")}` },
        inspector,
      );

      // Number: the numbering panel.
      await t.click({ role: "button", name: msg("inspector.numbering") }, inspector);
      // Seats from 101: the second "Start at" is the seat labels' (rows come first).
      await t.enter({ role: "textbox", name: msg("numbering.start"), nth: 1 }, "101", inspector);
      await t.key("Tab");
      await until("the preview to read A 101 … 110", async () =>
        /A 101 102 .* 110/.test(await t.text(inspector)),
      );
      await t.click({ role: "button", name: msg("numbering.apply") }, inspector);
      await until("the current labels to be the applied ones", async () => {
        const text = await t.text(inspector);
        return text.includes(msg("numbering.current").toUpperCase()) ||
          text.includes(msg("numbering.current"))
          ? /A 101 102 .* 110/.test(text)
          : false;
      });

      // A category: added in the categories panel, assigned in the inspector.
      await t.click({ role: "tab", name: msg("panel.categories") });
      const categories = await t.find({ role: "tabpanel", name: msg("panel.categories") });
      await t.click({ role: "button", name: msg("categories.add") }, categories);
      // The new category opens for editing: its label and key, each committed with Enter.
      await t.enter({ role: "textbox", name: msg("categories.labelField") }, "Stalls", categories);
      await t.key("Enter");
      await t.enter({ role: "textbox", name: msg("categories.key") }, "stalls", categories);
      await t.key("Enter");
      await t.find({ role: "button", name: /^Stalls: / }, categories);
      await t.click({ role: "button", name: BACK }, inspector);
      await t.click({ role: "button", name: new RegExp(`^${msg("field.category")} `) }, inspector);
      await t.click({ role: "option", name: "Stalls" });
      await t.find({ role: "button", name: `${msg("field.category")} Stalls` }, inspector);
      await until("the category to count 40 seats", async () =>
        (await t.text(categories)).includes(msg("categories.objects", { n: 40 })),
      );

      // Validation: the panel, once the check has run, lists no error.
      // Its name carries the count of findings once there are any.
      await t.click({ role: "tab", name: new RegExp(`^${msg("panel.validation")}( \\d+)?$`) });
      const panel = await t.find({
        role: "tabpanel",
        name: new RegExp(`^${msg("panel.validation")}`),
      });
      // Finished: the panel no longer says it is checking (the counts shown
      // meanwhile may be the last full check's, from before the last edit).
      const findings = await until(
        "validation to finish (the panel no longer checking)",
        async () => {
          const text = await t.text(panel);
          return text.includes(msg("validation.checking")) ? undefined : text;
        },
        30_000,
      );
      const problems =
        (await t.all({ role: "button", name: /./ }, bar)).find(
          (b) => b.name === msg("status.noProblems") || /errors · .* warnings$/.test(b.name),
        )?.name ?? "no summary in the status bar";
      const noErrors = msg("status.problems", { errors: 0, warnings: 0 }).split(" · ")[0] ?? "";
      check(
        problems === msg("status.noProblems") || problems.startsWith(`${noErrors} · `),
        "no validation errors",
        `${problems}: ${findings}`,
      );
      r.notes.push(`validation: ${problems}`);

      // Saved: the top bar's save state, then the draft on the server.
      const banner = await t.find({ role: "banner" });
      await until(
        "the top bar to say Saved",
        async () => {
          const states = await t.all({ role: "status" }, banner);
          return (
            states.some((s) => s.name.startsWith(msg("save.saved"))) ||
            (await Promise.all(states.map((s) => t.text(s.id)))).some((x) =>
              x.startsWith(msg("save.saved")),
            )
          );
        },
        30_000,
      );
      // What the server holds is what was drawn.
      const draft = await consoleApi<Draft>(t, `/v1/charts/${chart.key}/draft`);
      const block = draft.nodes.find((n) => n.type === "rowBlock");
      const found = {
        blocks: draft.nodes.length,
        rows: block?.rows?.length,
        seats: block?.rows?.map((row) => row.seats),
        curve: block?.curve,
        category: block?.category,
        start: block?.seatLabels?.start,
        categories: draft.categories.map((c) => `${c.key}:${c.label}`),
      };
      check(
        JSON.stringify(found) ===
          JSON.stringify({
            blocks: 1,
            rows: 4,
            seats: [10, 10, 10, 10],
            curve: { kind: "arc", radius: 1500 },
            category: "stalls",
            start: 101,
            // The server's empty draft's own, and the one added here.
            categories: ["standard:Standard", "stalls:Stalls"],
          }),
        "the saved draft: one block of 4 rows of 10, an arc of radius 1500, seats from 101, category stalls",
        found,
      );
    },
  },
  {
    name: "Publish from the designer; the version on the chart's page",
    async run(r) {
      const t = page(r, "owner");
      const banner = await t.find({ role: "banner" });
      await t.click({ role: "button", name: msg("topbar.publish") }, banner);
      const dialog = await t.find({
        role: DIALOG,
        name: msg("publish.title", { name: chart.name }),
      });
      await t.click({ role: "button", name: msg("publish.action") }, dialog);
      const done = msg("publish.done", { version: 1, seats: 40 });
      await until(
        `the dialog to say “${done}”`,
        async () => (await t.text(dialog)).includes(done),
        30_000,
      );
      await t.click({ role: "button", name: msg("action.close") }, dialog);
      await t.gone({ role: DIALOG });
      // Back to the console by the designer's menu, then the chart's page from the list.
      await t.click({ role: "button", name: msg("menu.label") }, banner);
      await t.click({ role: "menuitem", name: msg("menu.console") });
      await t.click({ role: "link", name: chart.name });
      const versions = await t.find({ role: "table", name: msg("chart.versions") });
      await until("version 1, current, with 40 seats, in the versions table", async () => {
        const text = await t.text(versions);
        return (
          text.includes(msg("events.versionShort", { number: 1 })) &&
          text.includes(msg("chart.current")) &&
          /\b40\b/.test(text)
        );
      });
    },
  },
  {
    name: "Create an event on that version from the console's form",
    async run(r) {
      const t = page(r, "owner");
      const versions = await t.find({ role: "table", name: msg("chart.versions") });
      await t.click({ role: "button", name: msg("chart.createEvent") }, versions);
      const form = await t.find({
        role: DIALOG,
        name: msg("events.newFromTitle", { name: chart.name }),
      });
      await t.enter({ role: "textbox", name: msg("events.name") }, event.name, form);
      const key = await t.value(await t.find({ role: "textbox", name: msg("events.key") }, form));
      check(key === event.key, `the key made from the name, ${event.key}`, key);
      // The version the row was for is the one chosen.
      const version = await t.value(
        await t.find({ role: "combobox", name: msg("events.version") }, form),
      );
      check(version === "1", "version 1 chosen", version);
      await t.click({ role: "button", name: msg("events.create") }, form);
      await t.gone({ role: DIALOG });
      await t.find({ role: "status", name: msg("events.created", { name: event.name }) });
    },
  },
  {
    name: "Keys: a secret and a publishable key from the API keys screen, each read once",
    async run(r) {
      const t = page(r, "owner");
      await nav(t, "nav.developers");
      const make = async (kind: "secret" | "publishable", name: string): Promise<string> => {
        await t.click({ role: "button", name: msg("keys.new") });
        const form = await t.find({ role: DIALOG, name: msg("keys.newTitle") });
        await t.enter({ role: "textbox", name: msg("keys.name") }, name, form);
        const label = msg(kind === "secret" ? "keys.secret" : "keys.publishable");
        await t.click({ role: "radio", name: new RegExp(`^${label}`) }, form);
        if (kind === "publishable") {
          // Only the buyer's page may use it.
          await t.enter({ role: "textbox", name: msg("keys.origins") }, r.buyer, form);
        }
        await t.click({ role: "button", name: msg("keys.create") }, form);
        const shown = await t.find({ role: DIALOG, name: msg("keys.shownTitle", { name }) });
        const value = await t.value(await t.find({ role: "textbox", name: label }, shown));
        await t.click({ role: "button", name: msg("action.close") }, shown);
        await t.gone({ role: DIALOG });
        return value;
      };
      keys.secret = await make("secret", "Box office backend");
      r.secrets.set("secret key", keys.secret);
      keys.publishable = await make("publishable", "Buyer page");
      r.secrets.set("publishable key", keys.publishable);
      check(keys.secret.startsWith("sk_"), "a secret key (sk_…)", keys.secret.slice(0, 3));
      check(
        keys.publishable.startsWith("pk_"),
        "a publishable key (pk_…)",
        keys.publishable.slice(0, 3),
      );
      // Both work: the secret reads the event, which the console made on version 1.
      const e = await api<{ version: { number: number }; counts: Record<string, number> }>(
        r,
        "GET",
        `/v1/events/${event.key}`,
      );
      check(
        e.version.number === 1 && e.counts["free"] === 40,
        "the event on version 1 with 40 free seats",
        e,
      );
    },
  },
  {
    name: "Buy: two buyers on another origin; a click holds a seat the other sees taken; the backend books it",
    async run(r) {
      const url = `${r.buyer}/?${new URLSearchParams({ server: r.base, key: keys.publishable, event: event.key })}`;
      const one = await tab(r, "buyer-1");
      const two = await tab(r, "buyer-2");
      for (const b of [one, two]) {
        await b.goto(url);
        await until(
          `the map on ${b.name}`,
          () => b.evaluate<boolean>("window.buyer.map !== null"),
          30_000,
        );
      }
      // The server has no licence: each map carries the "Development licence"
      // mark (12 §7.3), after its first frame; the clicks below go through it.
      for (const b of [one, two]) {
        await until(`the "Development licence" mark on ${b.name}'s map`, () =>
          b.evaluate<boolean>(
            `document.getElementById("map")?.shadowRoot?.querySelector(".w")?.textContent === "Development licence"`,
          ),
        );
      }
      // Realtime over the WebSocket, as `caveaServer` prefers it (05 §4.1):
      // each map's socket answered (101) and is sending frames.
      for (const b of [one, two]) {
        await until(`${b.name}'s WebSocket to the server, open and receiving`, async () =>
          liveSocket(b),
        );
      }
      const free = await api<{ data: Array<{ label: string }> }>(
        r,
        "GET",
        `/v1/events/${event.key}/objects?status=free&limit=100`,
      );
      check(free.data.length === 40, "40 free seats", free.data.length);
      // A seat in the middle of the block: row B, the fifth seat, numbered from 101.
      seat.label = "-B-105";
      check(
        free.data.some((o) => o.label === seat.label),
        `${seat.label} among the free seats`,
        free.data.map((o) => o.label),
      );
      const [x, y] = await seatOnScreen(one, seat.label);
      const holds: number[] = [];
      const releases: number[] = [];
      for (let i = 0; i < LATENCY_RUNS; i++) {
        for (const [want, list] of [
          ["unavailable", holds],
          ["available", releases],
        ] as const) {
          if (want === "available" && i === LATENCY_RUNS - 1) break;
          await two.evaluate(`window.buyer.watch(${JSON.stringify(seat.label)}, "${want}")`);
          const pressed = await one.evaluate<number>("window.buyer.presses.length");
          await tap(r, one, x, y);
          const seen = await until(`buyer 2's map to show ${seat.label} ${want}`, () =>
            two.evaluate<number | null>(`window.buyer.seen[${JSON.stringify(seat.label)}] ?? null`),
          );
          const press = await one.evaluate<number>(`window.buyer.presses[${pressed}]`);
          list.push(seen - press);
          // Buyer 1's own map settles (held, or released) before the next click.
          const selected = want === "unavailable";
          await until(
            `buyer 1's selection to ${selected ? "hold" : "drop"} ${seat.label}`,
            async () => {
              const s = await one.evaluate<{ labels: string[]; holdToken: string | null }>(
                "window.buyer.selection",
              );
              return (
                s.labels.includes(seat.label) === selected && (!selected || s.holdToken !== null)
              );
            },
          );
          if (selected) {
            await until(
              `the server to hold ${seat.label}`,
              async () => (await status(r, seat.label)) === "held",
            );
          } else {
            await until(
              `the server to free ${seat.label}`,
              async () => (await status(r, seat.label)) === "free",
            );
          }
        }
      }
      // No fallback to EventSource or polling, and no reconnect but the one
      // 05 §4.1 makes: buyer 1's socket follows its new hold token at once.
      for (const [b, most] of [
        [one, 2],
        [two, 1],
      ] as const) {
        const sockets = [...b.sockets.values()].filter((w) => w.url.includes("/v1/public/rt"));
        check(
          sockets.length <= most && liveSocket(b) && sockets.filter((w) => !w.closed).length === 1,
          `${b.name}: ${most === 1 ? "its one WebSocket" : "at most two WebSockets (the reconnect with the hold token)"}, one open`,
          sockets.map(
            (w) => `${w.status ?? "no answer"}, ${w.frames} frames${w.closed ? ", closed" : ""}`,
          ),
        );
        // One state read at the start, then the socket: EventSource is
        // `/stream`, polling more reads of `/state`.
        const reads = b.network.filter((line) =>
          /^\d+ GET \S+\/v1\/public\/events\/[^/]+\/(state|stream)\b/.test(line),
        );
        check(
          reads.length === 1 && reads[0]?.includes("/state") === true,
          `${b.name}: one state read and no EventSource or polling`,
          reads,
        );
      }
      r.latency.push(...holds);
      r.notes.push(
        `two-browser latency (WebSocket), click on buyer 1 to buyer 2's frame showing it: hold ${spread(holds)}; release ${spread(releases)}; in order, hold ${holds.map((v) => v.toFixed(0)).join(", ")}; release ${releases.map((v) => v.toFixed(0)).join(", ")}`,
      );
      seat.hold = (await one.evaluate<{ holdToken: string }>("window.buyer.selection")).holdToken;
      r.secrets.set("hold token", seat.hold);
      // The customer's backend books what the buyer holds.
      await api(r, "POST", `/v1/events/${event.key}/book`, {
        hold_token: seat.hold,
        order_ref: "e2e-order-1",
      });
      check(
        (await status(r, seat.label)) === "booked",
        `${seat.label} booked`,
        await status(r, seat.label),
      );
      check(
        (await two.evaluate<string>(
          `window.buyer.map.findObject(${JSON.stringify(seat.label)}).status`,
        )) === "unavailable",
        "the booked seat unavailable on buyer 2's map",
        "",
      );
    },
  },
  {
    name: "Operate: the manager shows the booking; blocks two seats; releases the booking",
    async run(r) {
      const t = page(r, "owner");
      await nav(t, "nav.events");
      await t.click({ role: "link", name: event.name });
      const region = await t.find(
        { role: "region", name: msg("manager.map", { name: event.name }) },
        undefined,
        30_000,
      );
      const details = await t.find({ role: "region", name: msg("manager.details") });
      const selection = await t.find({ role: "region", name: /^Selection/ });
      const canvas = await t.find({ role: "application" }, region);
      const two = page(r, "buyer-2");
      const [row, number] = place(seat.label);

      // The booked seat: its details say booked.
      await focusSeat(t, canvas, row, number);
      await until(`the details to show ${seat.label} booked`, async () => {
        const text = await t.text(details);
        return (
          text.includes(
            msg("manager.place", { section: msg("manager.none"), row, seat: number }),
          ) && text.includes(msg("manager.status.booked"))
        );
      });

      // Block two free seats behind it: row C, the same two numbers.
      const behind = String.fromCharCode(row.charCodeAt(0) + 1);
      const blocked = [`-${behind}-${number}`, `-${behind}-${Number(number) + 1}`];
      for (const label of blocked) {
        const [rw, n] = place(label);
        await focusSeat(t, canvas, rw, n);
        await t.key("Enter");
        await t.find({ role: "button", name: msg("manager.deselect", { label }) }, selection);
        await two.evaluate(`window.buyer.watch(${JSON.stringify(label)}, "unavailable")`);
      }
      await t.click({ role: "button", name: msg("manager.block") }, selection);
      await t.find({ role: "status", name: msg("manager.done.block", { n: 2 }) });
      for (const label of blocked) {
        check(
          (await status(r, label)) === "blocked",
          `${label} blocked on the server`,
          await status(r, label),
        );
        await until(`buyer 2's map to show ${label} unavailable`, () =>
          two.evaluate<boolean>(`window.buyer.seen[${JSON.stringify(label)}] !== undefined`),
        );
      }

      // Release the booking, after the confirmation.
      await focusSeat(t, canvas, row, number);
      await t.key("Enter");
      await t.find(
        { role: "button", name: msg("manager.deselect", { label: seat.label }) },
        selection,
      );
      await two.evaluate(`window.buyer.watch(${JSON.stringify(seat.label)}, "available")`);
      await t.click({ role: "button", name: new RegExp(`^${msg("manager.release")}`) }, selection);
      const confirm = await t.find({ role: DIALOG, name: msg("manager.releaseTitle", { n: 1 }) });
      await t.click({ role: "button", name: msg("manager.releaseConfirm") }, confirm);
      await t.find({ role: "status", name: msg("manager.done.release", { n: 1 }) });
      check(
        (await status(r, seat.label)) === "free",
        `${seat.label} free on the server`,
        await status(r, seat.label),
      );
      await until(`buyer 2's map to show ${seat.label} available again`, () =>
        two.evaluate<boolean>(`window.buyer.seen[${JSON.stringify(seat.label)}] !== undefined`),
      );
    },
  },
  {
    name: "People: a viewer invited, the link opened elsewhere, a password set; the map without actions",
    async run(r) {
      const t = page(r, "owner");
      r.secrets.set("viewer email", viewer.email);
      r.secrets.set("viewer password", viewer.password);
      // The event page is a full-window tool: back to the console's frame first.
      await t.click({ role: "link", name: msg("event.back") });
      await nav(t, "nav.settings");
      await t.click({ role: "tab", name: msg("settings.members") });
      await t.click({ role: "button", name: msg("invite.new") });
      const form = await t.find({ role: DIALOG, name: msg("invite.title") });
      await t.enter({ role: "textbox", name: msg("invite.email") }, viewer.email, form);
      // The role is a native select: from the email field, Tab to it and type the role's first letter.
      await t.key("Tab");
      const role = await t.find({ role: "combobox", name: msg("invite.role") }, form);
      check(await focused(t, role), "the role field focused after the email", "another element");
      await t.type(msg("role.viewer").slice(0, 1).toLowerCase());
      check((await t.value(role)) === "viewer", "the role viewer chosen", await t.value(role));
      await t.click({ role: "button", name: msg("invite.create") }, form);
      const shown = await t.find({
        role: DIALOG,
        name: msg("invite.shownTitle", { email: viewer.email }),
      });
      const link = await t.value(
        await t.find({ role: "textbox", name: msg("invite.link") }, shown),
      );
      // The token alone, as the page sends it to the server.
      r.secrets.set("invitation token", link.split("#token=")[1] ?? link);
      check(
        link.startsWith(`${r.base}/`) && link.includes("#token="),
        `an invitation link on ${r.base}`,
        link.split("#")[0],
      );
      await t.click({ role: "button", name: msg("action.close") }, shown);

      // A fresh browser: nobody signed in.
      const v = await tab(r, "viewer");
      v.expect("GET", /^\/console\/api\/me$/, 401);
      await v.goto(link);
      await v.find({ role: "heading", name: msg("accept.title") });
      await v.enter({ role: "textbox", name: msg("setup.password") }, viewer.password);
      await v.enter({ role: "textbox", name: msg("setup.confirm") }, viewer.password);
      await v.click({ role: "button", name: msg("accept.submit") });
      await v.find(
        { role: "button", name: msg("user.menu", { email: viewer.email }) },
        undefined,
        30_000,
      );
      await keepSession(r, v, "viewer");
      await nav(v, "nav.events");
      await v.click({ role: "link", name: event.name });
      const region = await v.find({
        role: "region",
        name: msg("manager.map", { name: event.name }),
      });
      await v.find({ role: "application" }, region);
      const selection = await v.find({ role: "region", name: /^Selection/ });
      await until("the viewer's note that their role cannot act", async () =>
        (await v.text(selection)).includes(msg("manager.roleView", { role: msg("role.viewer") })),
      );
      for (const action of [
        "manager.block",
        "manager.unblock",
        "manager.book",
        "manager.release",
      ] as const) {
        check(
          !(await v.has({ role: "button", name: new RegExp(`^${msg(action)}`) }, selection)),
          `no ${msg(action)} button for a viewer`,
          "one",
        );
      }
    },
  },
  {
    name: "Two-step: the viewer turns it on from the key shown, signs out, and signs in with a code",
    async run(r) {
      const v = page(r, "viewer");
      // From the event page (a full-window tool) back to the console's frame.
      await v.click({ role: "link", name: msg("event.back") });
      await v.click({ role: "button", name: msg("user.menu", { email: viewer.email }) });
      await v.click({ role: "menuitem", name: msg("user.account") });
      await v.click({ role: "button", name: msg("account.twoStepTurnOn") });
      const d = await v.find({ role: DIALOG, name: msg("totp.title") });
      await v.enter({ role: "textbox", name: msg("totp.password") }, viewer.password, d);
      await v.click({ role: "button", name: msg("totp.start") }, d);
      await v.find({ role: "image", name: msg("totp.qrLabel") }, d);
      const key = (
        await v.value(await v.find({ role: "textbox", name: msg("totp.secret") }, d))
      ).replaceAll(" ", "");
      check(/^[A-Z2-7]{32}$/.test(key), "a 160-bit base32 key shown as text", key.length);
      r.secrets.set("viewer's authenticator key", key);
      const step = Math.floor(Date.now() / 30_000);
      await v.enter({ role: "textbox", name: msg("totp.code") }, totp(key, step), d);
      await v.click({ role: "button", name: msg("totp.confirm") }, d);
      const codes = await v.find({ role: DIALOG, name: msg("codes.title") });
      const list = await v.find({ role: "list", name: msg("codes.list") }, codes);
      const shown = (await v.text(list)).match(/[a-z2-7]{5}-[a-z2-7]{5}/g) ?? [];
      check(shown.length === 10, "ten recovery codes", shown.length);
      r.secrets.set("viewer's recovery codes", shown.join("\n"));
      await v.click({ role: "checkbox", name: msg("codes.saved") }, codes);
      await v.click({ role: "button", name: msg("codes.done") }, codes);
      await v.gone({ role: DIALOG, name: msg("codes.title") });
      await v.find({ role: "button", name: msg("account.twoStepTurnOff") });

      await v.click({ role: "button", name: msg("user.menu", { email: viewer.email }) });
      await v.click({ role: "menuitem", name: msg("user.signOut") });
      await v.find({ role: "heading", name: msg("signIn.title") });
      await v.enter({ role: "textbox", name: msg("signIn.email") }, viewer.email);
      await v.enter({ role: "textbox", name: msg("signIn.password") }, viewer.password);
      await v.click({ role: "button", name: msg("signIn.submit") });
      // The next step's code: the server takes one step either way, and
      // never a step it took before.
      await v.enter({ role: "textbox", name: msg("signIn.code") }, totp(key, step + 1));
      await v.click({ role: "button", name: msg("signIn.codeSubmit") });
      await v.find({ role: "button", name: msg("user.menu", { email: viewer.email }) });
    },
  },
  {
    name: "Restart: stopped and started on the same database; bookings, chart and members kept",
    async run(r) {
      // Pages holding live connections would see the server go: closed first,
      // as a planned restart's maintenance window would have them.
      for (const name of ["buyer-1", "buyer-2", "viewer"]) {
        await r.tabs.get(name)?.close();
        r.tabs.delete(name);
      }
      const t = page(r, "owner");
      await t.goto("about:blank");
      const before = await api<{ counts: Record<string, number> }>(
        r,
        "GET",
        `/v1/events/${event.key}`,
      );
      const stopped = await r.server.stop();
      check(
        stopped.code === 0 && stopped.seconds < 30,
        "a clean stop (exit 0) within 30 s of SIGTERM",
        `exit ${stopped.code} after ${stopped.seconds.toFixed(1)} s`,
      );
      check(
        /\bstopped\b/.test(r.server.log.split("\n").slice(-5).join("\n")),
        "the log's last lines to say stopped",
        r.server.log.slice(-500),
      );
      await r.server.start();
      r.notes.push(`restart: stopped in ${stopped.seconds.toFixed(2)} s`);
      // Seats: the counts as they were (two blocked, the booking released).
      const after = await api<{ counts: Record<string, number> }>(
        r,
        "GET",
        `/v1/events/${event.key}`,
      );
      check(
        JSON.stringify(after.counts) === JSON.stringify(before.counts) &&
          after.counts["blocked"] === 2,
        `the seat counts kept: ${JSON.stringify(before.counts)}`,
        after.counts,
      );
      // The chart: still published, version 1 of 40 seats.
      const c = await api<{
        published_version: { number: number; stats: { seats: number } } | null;
      }>(r, "GET", `/v1/charts/${chart.key}`);
      check(
        c.published_version?.number === 1 && c.published_version.stats.seats === 40,
        "the chart published at version 1 with 40 seats",
        c.published_version,
      );
      // The members, in the console, with the owner's session from before the restart.
      await t.goto(`${r.base}/settings`);
      await t.click({ role: "tab", name: msg("settings.members") });
      const members = await t.find({ role: "table", name: msg("members.title") });
      await until("the owner and the viewer in the members table", async () => {
        const text = await t.text(members);
        return (
          text.includes(owner.email) &&
          text.includes(viewer.email) &&
          text.includes(msg("role.viewer"))
        );
      });
    },
  },
  {
    name: "Throughout: no secret in the server's log",
    async run(r) {
      const log = r.server.log;
      const found = [...r.secrets]
        .filter(([, value]) => value !== "" && log.includes(value))
        .map(([what]) => what);
      check(
        found.length === 0,
        `none of ${[...r.secrets.keys()].join(", ")} in the log`,
        found.join(", "),
      );
      check(r.secrets.size >= 12, "every secret of the run collected for the check", [
        ...r.secrets.keys(),
      ]);
    },
  },
];
