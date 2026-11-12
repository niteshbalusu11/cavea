// The designer guide's tasks, page by page, in the order the guide gives
// them, each done in the real designer by the labels the page names. Run by
// `check.ts`, which starts the server and the browser and signs in.
import { join } from "node:path";
import type { Browser } from "../../../tools/chromium.ts";
import { Tab, until } from "../../../tools/e2e/tab.ts";
import { msg } from "../../console/src/i18n/messages.ts";

export interface Guide {
  t: Tab;
  browser: Browser;
  base: string;
  out: string;
  shoot: (t: Tab, name: string, clip?: Clip, scale?: number) => Promise<void>;
  look: (t: Tab, name: string) => Promise<void>;
  log: (text: string) => void;
  owner: { email: string; password: string };
}

export interface Task {
  name: string;
  run: (g: Guide) => Promise<void>;
}

interface Clip {
  x: number;
  y: number;
  width: number;
  height: number;
}

const DIALOG = "dialog|alertdialog";
export const CHART = { name: "Riverside Hall", key: "riverside-hall" };
/** Product bugs the run met and worked around as the guide says: reported at the end. */
export const KNOWN: string[] = [];

function check(ok: unknown, what: string): asserts ok {
  if (!ok) throw new Error(`designer guide: ${what}`);
}

// ------------------------------------------------------------- helpers

/** Modifier bits of the DevTools protocol. */
const CTRL = 2;

/** A key with modifiers, as a person presses it: `chord(t, "2", CTRL)` is Ctrl+2. */
export async function chord(t: Tab, key: string, modifiers = 0): Promise<void> {
  const upper = key.toUpperCase();
  const code = /^[a-z]$/i.test(key) ? `Key${upper}` : /^[0-9]$/.test(key) ? `Digit${key}` : key;
  const text = modifiers === 0 && key.length === 1 ? key : undefined;
  const vk = key === "Delete" ? 46 : upper.charCodeAt(0);
  const base = { key, code, modifiers, windowsVirtualKeyCode: vk };
  await t.send("Input.dispatchKeyEvent", {
    type: text === undefined ? "rawKeyDown" : "keyDown",
    ...base,
    ...(text === undefined ? {} : { text }),
  });
  await t.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
}

/** The box of the elements, grown by `pad` px each side, kept within the page. */
async function around(t: Tab, ids: number[], pad = 8): Promise<Clip> {
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
  return { x, y, width: Math.min(1440, r + pad) - x, height: Math.min(900, b + pad) - y };
}

/** A file input's files, as the picker its button opens would set them. */
async function pick(t: Tab, selector: string, path: string): Promise<void> {
  const { root } = (await t.send("DOM.getDocument", { depth: 0 })) as { root: { nodeId: number } };
  const { nodeId } = (await t.send("DOM.querySelector", { nodeId: root.nodeId, selector })) as {
    nodeId: number;
  };
  check(nodeId !== 0, `a file input ${selector}`);
  await t.send("DOM.setFileInputFiles", { files: [path], nodeId });
}

/** The canvas element and its box. */
async function canvas(t: Tab): Promise<{ id: number; box: [number, number, number, number] }> {
  const id = await t.find({ role: "application", name: msg("canvas.label", { name: CHART.name }) });
  return { id, box: await t.box(id) };
}

const statusBar = (t: Tab): Promise<number> => t.find({ role: "contentinfo" });
const inspector = (t: Tab): Promise<number> => t.find({ role: "region", name: msg("panel.inspector") });

/** Waits until the status bar's text matches. */
async function status(t: Tab, what: string, test: (text: string) => boolean): Promise<string> {
  const bar = await statusBar(t);
  return until(`the status bar to show ${what}`, async () => {
    const text = await t.text(bar);
    return test(text) ? text : undefined;
  });
}

/** A field of the inspector set as a person does: typed over, then Enter. */
async function field(t: Tab, name: string, value: string): Promise<void> {
  await t.enter({ role: "textbox", name }, value, await inspector(t));
  await t.key("Enter");
}

/** A choice of the inspector (a select button), then its option. */
async function choose(t: Tab, name: string, option: string, within?: number): Promise<void> {
  await t.click({ role: "button", name: new RegExp(`^${name} `) }, within ?? (await inspector(t)));
  await t.click({ role: "option", name: option });
  await t.find({ role: "button", name: `${name} ${option}` }, within ?? (await inspector(t)));
}

/** Closes the notices in the corner, so pictures show only the designer. */
async function quiet(t: Tab): Promise<void> {
  const region = (await t.all({ role: "region", name: "Cavea" }))[0];
  if (region === undefined) return;
  for (const n of await t.all({ role: "button", name: msg("action.close") }, region.id)) {
    await t.clickAt(...(await t.centre(n.id))).catch(() => undefined);
  }
}

/** The command palette: a command by its name. */
async function command(t: Tab, name: string): Promise<void> {
  await chord(t, "k", CTRL);
  const palette = await t.find({ role: DIALOG, name: msg("palette.title") });
  await t.type(name.slice(0, 12));
  await t.click({ role: "option", name: new RegExp(`^${name}`) }, palette);
  await t.gone({ role: DIALOG, name: msg("palette.title") });
}

/** The top bar's save state reads `prefix` (Saved, Offline …). */
async function saveState(t: Tab, what: string, ms = 30_000): Promise<string> {
  const banner = await t.find({ role: "banner" });
  return until(
    `the top bar to say ${what}`,
    async () => {
      const text = await t.text(banner);
      return text.includes(what) ? text : undefined;
    },
    ms,
  );
}

/** The chart's draft on the server, read with the console's own session. */
async function draft(t: Tab): Promise<Record<string, unknown>> {
  return t.evaluate<Record<string, unknown>>(`(async () => {
    const me = await (await fetch("/console/api/me")).json();
    const r = await fetch("/v1/charts/${CHART.key}/draft", { headers: { "cavea-workspace": me.workspaces[0].id } });
    return r.json();
  })()`);
}

/** A floor plan as a picture file, drawn here, as a venue's would be scanned. */
async function floorPlan(g: Guide): Promise<string> {
  // To scale, one pixel a centimetre: rows 95 apart, 14 seats 55 apart, as the
  // designer's defaults space them.
  const rows = Array.from({ length: 8 }, (_, i) => {
    const y = 380 + i * 95;
    return `<path d="M415 ${y} Q800 ${y + 60} 1185 ${y}"/>`;
  }).join("");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1300" viewBox="0 0 1600 1300">
  <rect width="1600" height="1300" fill="#fff"/>
  <path d="M60 60 H1540 V1240 H900 M700 1240 H60 Z" fill="none" stroke="#333" stroke-width="8"/>
  <rect x="400" y="80" width="800" height="180" fill="#ddd" stroke="#333" stroke-width="4"/>
  <text x="800" y="185" font-family="sans-serif" font-size="56" text-anchor="middle">STAGE</text>
  <g stroke="#999" stroke-width="3" fill="none">${rows}</g>
  <text x="800" y="1285" font-family="sans-serif" font-size="36" text-anchor="middle">ENTRANCE</text>
</svg>`;
  const t = await Tab.open(g.browser, "floor-plan", 1600, 1300);
  await t.goto(`data:image/svg+xml,${encodeURIComponent(svg)}`);
  const { data } = (await t.send("Page.captureScreenshot", { format: "png" })) as { data: string };
  await t.close();
  const path = join(g.out, "floor-plan.png");
  await Bun.write(path, Buffer.from(String(data), "base64"));
  return path;
}

/**
 * Chart units to page pixels, as the status bar's pointer readout gives
 * them at two points of the canvas.
 */
async function calibrate(t: Tab): Promise<(x: number, y: number) => [number, number]> {
  const { box } = await canvas(t);
  const read = async (px: number, py: number): Promise<[number, number]> => {
    await t.mouse("mouseMoved", px, py);
    const text = await status(t, "the pointer's position", (x) => /x -?[\d,.]+ +y -?[\d,.]+/.test(x));
    const m = text.match(/x (-?[\d,.]+) +y (-?[\d,.]+)/);
    return [Number((m?.[1] ?? "").replace(/,/g, "")), Number((m?.[2] ?? "").replace(/,/g, ""))];
  };
  const a: [number, number] = [box[0] + box[2] * 0.25, box[1] + box[3] * 0.25];
  const b: [number, number] = [box[0] + box[2] * 0.75, box[1] + box[3] * 0.75];
  const [ax, ay] = await read(...a);
  const [bx, by] = await read(...b);
  const sx = (b[0] - a[0]) / (bx - ax);
  const sy = (b[1] - a[1]) / (by - ay);
  return (x, y) => [a[0] + (x - ax) * sx, a[1] + (y - ay) * sy];
}

// --------------------------------------------------------------- tasks

export const TASKS: Task[] = [
  {
    name: "start.md: a new chart, opened in the designer; the tour",
    async run({ t, look, shoot }) {
      await t.click({ role: "button", name: msg("charts.new"), nth: 0 });
      const form = await t.find({ role: DIALOG, name: msg("charts.newTitle") });
      await t.enter({ role: "textbox", name: msg("charts.name") }, CHART.name, form);
      const key = await t.value(await t.find({ role: "textbox", name: msg("charts.key") }, form));
      check(key === CHART.key, `the key fills in as ${CHART.key}, not ${key}`);
      await shoot(t, "new-chart", await around(t, [form], 0));
      await t.click({ role: "button", name: msg("charts.create") }, form);
      await t.gone({ role: DIALOG });
      await t.click({ role: "link", name: CHART.name });
      await t.click({ role: "link", name: msg("charts.open") });
      await t.find(
        { role: "application", name: msg("canvas.label", { name: CHART.name }) },
        undefined,
        30_000,
      );
      const tour = await t.find({ role: "dialog", name: msg("tour.draw") });
      await quiet(t);
      await look(t, "tour");
      await shoot(t, "tour", { x: 0, y: 0, width: 360, height: 330 });
      check(
        (await t.text(tour)).includes(msg("tour.step", { n: 1, of: 5 })),
        "the tour starts at step 1 of 5",
      );
      await t.click({ role: "button", name: msg("tour.skip") }, tour);
      await t.gone({ role: "dialog", name: msg("tour.draw") });
    },
  },
  {
    name: "start.md: trace a floor plan with a reference picture",
    async run(g) {
      const { t, look, shoot } = g;
      const plan = await floorPlan(g);
      // The designer's tab in front again: a tab behind another draws nothing.
      await t.send("Page.bringToFront");
      await t.send("Page.setInterceptFileChooserDialog", { enabled: true });
      await t.click({ role: "button", name: msg("shortcut.image") });
      await pick(t, 'input[type=file][accept^="image/"]', plan);
      await t.send("Page.setInterceptFileChooserDialog", { enabled: false });
      const panel = await inspector(t);
      await until("the inspector to show the picture", async () =>
        (await t.text(panel)).includes(msg("image.referenceNote")),
      );
      await until("the picture to finish uploading", async () =>
        !(await t.text(await statusBar(t))).includes(msg("image.uploadingCount", { n: 1 })),
      );
      await chord(t, "0", CTRL);
      await Bun.sleep(500);
      await look(t, "reference");
      // Locked, so drawing over it never moves it.
      await t.click({ role: "checkbox", name: msg("field.locked") }, panel);
      await shoot(t, "reference", undefined, 0.5);
    },
  },
  {
    name: "find-your-way.md: zoom, pan, panels, theme, undo",
    async run({ t, look, shoot }) {
      const { box } = await canvas(t);
      const [left, top, width, height] = box;
      const mid: [number, number] = [left + width / 2, top + height / 2];
      const zoom = async (): Promise<string> =>
        (await t.nameOf({ role: "button", name: /^Zoom to fit/ }, await statusBar(t))).replace(
          /^.*\((.*)\)$/,
          "$1",
        );
      const before = await zoom();
      // Ctrl and the wheel zoom about the pointer; the wheel alone pans.
      await t.send("Input.dispatchMouseEvent", {
        type: "mouseWheel",
        x: mid[0],
        y: mid[1],
        deltaX: 0,
        deltaY: -100,
        modifiers: CTRL,
      });
      await until("Ctrl and the wheel to zoom in", async () => (await zoom()) !== before);
      await t.send("Input.dispatchMouseEvent", {
        type: "mouseWheel",
        x: mid[0],
        y: mid[1],
        deltaX: 0,
        deltaY: 200,
      });
      // The hand tool drags the view.
      await t.click({ role: "button", name: msg("tool.hand") });
      await t.drag(mid, [mid[0] + 120, mid[1] + 60]);
      await t.click({ role: "button", name: msg("tool.select") });
      // Zoom to fit: the status bar's percentage.
      await t.click({ role: "button", name: /^Zoom to fit/ }, await statusBar(t));
      await shoot(t, "status-bar", await around(t, [await statusBar(t)], 0));
      // The panels' parts, collapsed and back.
      const side = await t.find({ role: "complementary" });
      await look(t, "screen");
      await t.click({ role: "button", name: msg("panel.collapse", { panel: msg("panel.inspector") }) });
      await t.click({ role: "button", name: msg("panel.expand", { panel: msg("panel.inspector") }) });
      void side;
      // Dark, then back to light, from the menu.
      const banner = await t.find({ role: "banner" });
      for (const theme of [msg("theme.dark"), msg("theme.light")]) {
        await t.click({ role: "button", name: msg("menu.label") }, banner);
        await t.click({ role: "menuitem", name: msg("menu.theme") });
        await t.click({ role: "menuitemradio", name: theme });
        // The menu stays open on a theme, to try another; Escape closes it.
        if (await t.has({ role: "menu" })) await t.key("Escape");
        await t.gone({ role: "menu" });
        await Bun.sleep(300);
      }
      // The whole screen, for the page that names its parts.
      await quiet(t);
      await shoot(t, "screen", undefined, 0.5);
    },
  },
  {
    name: "seats.md: a row block drawn to size, placed, curved, spaced",
    async run({ t, look, shoot }) {
      const bar = await statusBar(t);
      await t.click({ role: "button", name: msg("tool.rowBlock") });
      const { box } = await canvas(t);
      const [left, top, width, height] = box;
      const [x0, y0] = [left + width * 0.38, top + height * 0.4];
      await t.mouse("mouseMoved", x0, y0);
      await t.mouse("mousePressed", x0, y0);
      for (let i = 1; i <= 8; i++) await t.mouse("mouseMoved", x0 + i * 25, y0 + i * 15, 1);
      await status(t, "the live count of rows and seats", (x) => /\d+ rows × \d+ seats/.test(x));
      await t.type("8x14");
      await status(t, "the typed size", (x) => x.includes(msg("readout.entry", { entry: "8x14" })));
      await shoot(t, "typing-size", await around(t, [bar], 0));
      await t.key("Enter");
      await t.mouse("mouseReleased", x0 + 200, y0 + 120);
      await status(t, "112 seats", (x) => x.includes(msg("status.seats", { n: 112 })));
      // Into place over the plan's front row: the position is the front row's centre.
      await field(t, msg("field.x"), "0");
      await field(t, msg("field.y"), "-270");
      await t.click({ role: "button", name: msg("tool.select") });
      await chord(t, "2", CTRL);
      await Bun.sleep(500);
      await look(t, "block");
      // The curve handle: the small diamond just behind the middle of the
      // front row. The pointer does not change over it, so it is found as a
      // person finds it by eye: a press there starts a curve (the status
      // bar reads "Depth"); a press beside it is cancelled with Escape.
      const c = await canvas(t);
      const cx = c.box[0] + c.box[2] / 2;
      await shoot(t, "curve-handle", { x: cx - 240, y: c.box[1], width: 480, height: 160 });
      let handle: number | undefined;
      for (let y = c.box[1] + 30; y < c.box[1] + 90 && handle === undefined; y += 3) {
        await t.mouse("mouseMoved", cx, y);
        await t.mouse("mousePressed", cx, y);
        await t.mouse("mouseMoved", cx, y + 6, 1);
        if (/Depth [\d.,-]+ cu/.test(await t.text(await statusBar(t)))) {
          handle = y;
        } else {
          await t.key("Escape");
          await t.mouse("mouseReleased", cx, y + 6);
        }
      }
      check(handle !== undefined, "the curve handle in the middle of the front row");
      for (let i = 2; i <= 6; i++) await t.mouse("mouseMoved", cx, handle + i * 10, 1);
      await status(t, "the curve's depth", (x) => /Depth [\d.,]+ cu/.test(x));
      await t.mouse("mouseReleased", cx, handle + 60);
      const panel = await inspector(t);
      await t.find({ role: "button", name: `${msg("field.curve")} ${msg("curve.arc")}` }, panel);
      // Exactly the plan's curve: the radius typed.
      await field(t, msg("field.curveRadius"), "1300");
      await field(t, msg("field.seatPitch"), "55");
      await field(t, msg("field.rowPitch"), "95");
      await chord(t, "2", CTRL);
      await Bun.sleep(500);
      await quiet(t);
      // The whole plan, to see the block on it.
      await t.click({ role: "button", name: /^Zoom to fit/ }, await statusBar(t));
      await Bun.sleep(500);
      await shoot(t, "block", { x: 60, y: 40, width: 1080, height: 830 }, 0.5);
      // numbering.md's warning: outside a section, labels start with a dash,
      // and a Seat label without {section} drops it.
      const first = async (): Promise<string> => {
        await t.click({ role: "button", name: msg("inspector.numbering") }, await inspector(t));
        const p = await inspector(t);
        const text = await until("the numbering panel's full label", async () => {
          const x = await t.text(p);
          return x.includes(msg("numbering.fullLabel")) ? x : undefined;
        });
        await t.click({ role: "button", name: msg("panel.back") });
        return text.split(msg("numbering.fullLabel"))[1]?.trim().split(/\s/)[0] ?? "";
      };
      const dash = await first();
      check(dash === "-A-1", `a block outside a section labels its first seat -A-1, not ${dash}`);
      KNOWN.push("a seat outside a section gets a label starting with a dash");
      const template = async (value: string): Promise<void> => {
        await t.call((await canvas(t)).id, "function () { this.focus(); }");
        await t.key("Escape");
        await until("the chart settings in the inspector", async () =>
          (await t.text(await inspector(t))).includes(msg("inspector.settings")),
        );
        await field(t, msg("field.labelTemplate"), value);
        await t.click({ role: "treeitem", name: /^Row block/ }, await t.find({ role: "tree", name: msg("layers.label") }));
      };
      await template("{row}-{seat}");
      const plain = await first();
      check(plain === "A-1", `with the Seat label {row}-{seat}, the first seat is A-1, not ${plain}`);
      await template("{section}-{row}-{seat}");
    },
  },
  {
    name: "seats.md: the Insert row block form",
    async run({ t, shoot, look }) {
      const banner = await t.find({ role: "banner" });
      await t.click({ role: "button", name: msg("menu.label") }, banner);
      await t.click({ role: "menuitem", name: msg("command.insertRowBlock") });
      const form = await t.find({ role: DIALOG, name: msg("insert.title") });
      // Its count of seats is the one it opened with: it does not follow the typing yet.
      await t.enter({ role: "textbox", name: msg("field.rows") }, "2", form);
      await t.key("Tab");
      await t.enter({ role: "textbox", name: msg("field.seats") }, "6", form);
      await t.key("Tab");
      await t.enter({ role: "textbox", name: msg("field.x") }, "-560", form);
      await t.key("Tab");
      await t.enter({ role: "textbox", name: msg("field.y") }, "470", form);
      await t.key("Tab");
      await t.enter({ role: "textbox", name: msg("field.rotation") }, "0", form);
      await t.key("Tab");
      await shoot(t, "insert-form", await around(t, [form], 0));
      // The guide's warning: the form's total stays at what it opened with.
      check(
        (await t.text(form)).includes(msg("insert.total", { n: 200 })),
        "the Insert row block form's total now follows the fields: remove the warning in seats.md",
      );
      KNOWN.push("the Insert row block form's total does not follow the fields");
      await t.click({ role: "button", name: msg("insert.submit") }, form);
      await t.gone({ role: DIALOG });
      await status(t, "124 seats", (x) => x.includes(msg("status.seats", { n: 124 })));
      // A product bug, written up in the guide: submitting the form can stop
      // the designer's canvas and panels following edits ("Stale read from
      // <Show>") until the page is reloaded; the block is saved. It does not
      // happen every time, so the page is reloaded either way, as the guide
      // says to.
      await t.collect();
      const stale = t.problems.filter((p) => p.text.includes("Stale read from <Show>"));
      t.problems.splice(0);
      KNOWN.push(
        `Insert row block: the designer stops following edits until a reload (${stale.length > 0 ? "it happened" : "it did not happen"} this run)`,
      );
      await saveState(t, msg("save.saved"));
      await t.goto(await t.evaluate<string>("location.href"));
      await t.find({ role: "application", name: msg("canvas.label", { name: CHART.name }) }, undefined, 30_000);
      await status(t, "124 seats after the reload", (x) => x.includes(msg("status.seats", { n: 124 })));
    },
  },
  {
    name: "arranging.md (sections first, for the labels): each block into a section with a label",
    async run({ t, look }) {
      const tree = await t.find({ role: "tree", name: msg("layers.label") });
      for (const [item, label] of [
        [/^Row block 2/, "Stalls"],
        [/^Row block 123/, "Box A"],
      ] as const) {
        const row = await t.find({ role: "treeitem", name: item }, tree);
        const [x, y] = await t.centre(row);
        await t.mouse("mouseMoved", x, y);
        await t.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "right", buttons: 2, clickCount: 1 });
        await t.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "right", buttons: 0, clickCount: 1 });
        await look(t, `context-${label}`);
        await t.click({ role: "menuitem", name: msg("layers.group") });
        await Bun.sleep(300);

        await look(t, `grouped-${label}`);
        await field(t, msg("field.label"), label);
      }
      await look(t, "sections");
    },
  },
  {
    name: "numbering.md: rows from J and odd seats in the box, the preview, Apply",
    async run({ t, look, shoot }) {
      const tree = await t.find({ role: "tree", name: msg("layers.label") });
      await t.click({ role: "treeitem", name: /^Box/ }, tree);
      const panel = await inspector(t);
      await t.click({ role: "button", name: msg("inspector.numbering") }, panel);
      await t.find({ role: "heading", name: msg("panel.numbering") }).catch(() => undefined);
      await t.enter({ role: "textbox", name: msg("numbering.start"), nth: 0 }, "J", panel);
      await t.key("Tab");
      await t.click({ role: "button", name: new RegExp(`^${msg("numbering.scheme")} `), nth: 1 }, panel);
      await t.click({ role: "option", name: msg("scheme.odd") });
      await until("the preview to read J 1 3 5 7 9 11", async () =>
        /J 1 3 5 7 9 11/.test(await t.text(panel)),
      );
      await chord(t, "2", CTRL);
      await Bun.sleep(500);
      await quiet(t);
      await look(t, "numbering");
      await shoot(t, "numbering", { x: 240, y: 40, width: 1200, height: 620 }, 0.7);
      await t.click({ role: "button", name: msg("numbering.apply") }, panel);
      await until("the box's labels to be the applied ones", async () =>
        /J 1 3 5 7 9 11/.test(await t.text(panel)) && (await t.text(panel)).includes(msg("numbering.untouched")),
      );
      await t.click({ role: "button", name: msg("panel.back") });
    },
  },
  {
    name: "categories.md: rename the first, add two, the similar-colour warning, the brush",
    async run({ t, look, shoot }) {
      await t.click({ role: "tab", name: msg("panel.categories") });
      const panel = await t.find({ role: "tabpanel", name: msg("panel.categories") });
      // The chart's first category, made with it: renamed for the stalls.
      await t.click({ role: "button", name: msg("categories.edit", { label: "Standard" }) }, panel);
      await t.enter({ role: "textbox", name: msg("categories.labelField") }, "Stalls", panel);
      await t.key("Enter");
      await t.enter({ role: "textbox", name: msg("categories.key") }, "stalls", panel);
      await t.key("Enter");
      await t.click({ role: "button", name: msg("categories.edit", { label: "Stalls" }) }, panel);
      const add = async (label: string, key: string, colour: string): Promise<void> => {
        await t.click({ role: "button", name: msg("categories.add") }, panel);
        await t.enter({ role: "textbox", name: msg("categories.labelField") }, label, panel);
        await t.key("Enter");
        await t.enter({ role: "textbox", name: msg("categories.key") }, key, panel);
        await t.key("Enter");
        await t.enter({ role: "textbox", name: msg("categories.colour") }, colour, panel);
        await t.key("Enter");
      };
      // A blue close to the stalls': the panel says so at once.
      await add("Box", "box", "#2A5FE0");
      const similar = msg("categories.similar", { others: "Stalls" });
      await until("the panel to warn the colours are hard to tell apart", async () =>
        (await t.text(panel)).includes(similar),
      );
      await look(t, "similar");
      await shoot(t, "similar", await around(t, [panel], 0));
      await t.enter({ role: "textbox", name: msg("categories.colour") }, "#B45309", panel);
      await t.key("Enter");
      await until("the warning to go", async () => !(await t.text(panel)).includes(similar));
      await t.click({ role: "button", name: msg("categories.edit", { label: "Box" }) }, panel);
      await add("Wheelchair space", "wheelchair", "#0F766E");
      await t.click({ role: "checkbox", name: msg("categories.accessible") }, panel);
      await t.click({ role: "button", name: msg("categories.edit", { label: "Wheelchair space" }) }, panel);
      // The box's seats: its section's category, in the inspector.
      await t.click({ role: "tab", name: msg("panel.layers") });
      const tree = await t.find({ role: "tree", name: msg("layers.label") });
      // A block's own category wins over its section's: the block it is, then.
      await t.click({ role: "button", name: /^Expand|^Box/ }, tree).catch(() => undefined);
      await t.click({ role: "treeitem", name: /^Box/ }, tree);
      await t.key("ArrowRight");
      await t.click({ role: "treeitem", name: /^Row block/ }, tree);
      await choose(t, msg("field.category"), "Box");
      // The brush: the wheelchair spaces at either end of the stalls' row A.
      // Single seats: a double-click steps into the block's rows, a second
      // one into the seats of the row clicked.
      await t.click({ role: "treeitem", name: /^Stalls/ }, tree);
      await chord(t, "2", CTRL);
      await Bun.sleep(500);
      await saveState(t, msg("save.saved"));
      const nodes = (await draft(t))["nodes"] as Array<{
        type: string;
        origin?: [number, number];
        children?: Array<{ origin: [number, number]; rows?: unknown[] }>;
      }>;
      const stalls = nodes.find((n) => n.type === "section" && n.children?.[0]?.rows?.length === 8);
      // A section's children are placed from the section's origin.
      const [sx, sy] = stalls?.origin ?? [0, 0];
      const [bx, by] = stalls?.children?.[0]?.origin ?? [0, 0];
      const [ox, oy] = [sx + bx, sy + by];
      // Row A's end seats: on an arc of radius 1,300 whose middle is the
      // block's origin, 6.5 seat pitches either side along it.
      const theta = 357.5 / 1300;
      const y = oy - 1300 * (1 - Math.cos(theta));
      const ends = [ox - 1300 * Math.sin(theta), ox + 1300 * Math.sin(theta)];
      const at = await calibrate(t);
      const a1 = at(ends[0] ?? 0, y);
      // From the section: its block, the block's rows, then the row's seats.
      for (const level of [msg("kind.rowBlock"), msg("inspector.level.rows"), msg("inspector.level.seats")]) {
        await t.clickAt(...a1);
        await t.clickAt(...a1);
        await until(`the inspector to say ${level}`, async () =>
          (await t.text(await inspector(t))).startsWith(`${msg("panel.inspector")} ${level} `),
        );
        await Bun.sleep(350);
      }
      // seats.md: Delete removes the selected seat; undone at once.
      await chord(t, "Delete");
      await status(t, "123 seats", (x) => x.includes(msg("status.seats", { n: 123 })));
      await chord(t, "z", CTRL);
      await status(t, "124 seats again", (x) => x.includes(msg("status.seats", { n: 124 })));
      await t.clickAt(...a1);
      await t.click({ role: "tab", name: msg("panel.categories") });
      await t.click({ role: "button", name: `Wheelchair space: ${msg("categories.makeActive")}` });
      await t.click({ role: "button", name: msg("tool.brush") });
      for (const x of ends) await t.clickAt(...at(x, y));
      await until("the wheelchair category to count 2 seats", async () => {
        const p = await t.find({ role: "tabpanel", name: msg("panel.categories") });
        return /Wheelchair space.*2 objects/.test(await t.text(p));
      });
      await t.click({ role: "button", name: msg("tool.select") });
      await quiet(t);
      await shoot(t, "brush", undefined, 0.6);
    },
  },
  {
    name: "arranging.md: the stage, the focal point, a second box by duplicating, align, layers, guides, snapping",
    async run({ t, look, shoot }) {
      // Out of the seats, the row, the block and the section, as the guide
      // says before drawing: inside a section, a shape drawn on it joins it.
      await t.call((await canvas(t)).id, "function () { this.focus(); }");
      for (let i = 0; i < 4; i++) await t.key("Escape");
      await chord(t, "0", CTRL);
      await Bun.sleep(500);
      const at = await calibrate(t);
      // The stage: a rectangle over the plan's, and a text on it.
      await t.click({ role: "button", name: msg("tool.rect") });
      await t.drag(at(-400, -570), at(400, -390), 10);
      await status(t, "the stage as a sixth object", (x) => x.includes(msg("status.nodes", { n: 6 })));
      await t.click({ role: "tab", name: msg("panel.layers") });
      check(
        (await t.has({ role: "treeitem", name: /^Box A 1/ })) &&
          (await t.has({ role: "treeitem", name: /^Stalls 1/ })),
        "the stage was drawn at the top level, not into a section",
      );
      // Two product gaps the guide states: the Text tool opens no field to
      // type in, and a chart started from scratch has no floor to hold a
      // focal point. Once either works, this fails: update arranging.md.
      await t.click({ role: "button", name: msg("tool.text") });
      await t.clickAt(...at(0, -480));
      await t.type("STAGE");
      await t.key("Enter");
      await t.click({ role: "button", name: msg("tool.focal") });
      await t.clickAt(...at(0, -480));
      await t.click({ role: "button", name: msg("tool.select") });
      await Bun.sleep(500);
      check(
        (await t.text(await statusBar(t))).includes(msg("status.nodes", { n: 6 })),
        "the Text tool now makes a text: update arranging.md",
      );
      KNOWN.push("the Text tool opens no field to type in");
      const floors = (await draft(t))["floors"] as unknown[];
      check(floors.length === 0, "a chart started from scratch now has a floor: update arranging.md and checking.md");
      KNOWN.push("a chart started from scratch has no floor, so no focal point");
      // A second box: Box A duplicated, moved across, its own label.
      await t.click({ role: "tab", name: msg("panel.layers") });
      const tree = await t.find({ role: "tree", name: msg("layers.label") });
      await t.click({ role: "treeitem", name: /^Box A/ }, await t.find({ role: "tree", name: msg("layers.label") }));
      await chord(t, "d", CTRL);
      await until("a copy of the box", async () =>
        (await t.all({ role: "treeitem", name: /^Box A/ }, await t.find({ role: "tree", name: msg("layers.label") }))).length === 2,
      );
      const x = await t.value(await t.find({ role: "textbox", name: msg("field.x") }, await inspector(t)));
      await field(t, msg("field.x"), String(-Number(x.replace(/,/g, "")) + 50));
      await field(t, msg("field.rotation"), "0");
      void tree;
      await look(t, "duplicated");
      await status(t, "136 seats", (x) => x.includes(msg("status.seats", { n: 136 })));
      // Its labels are the original's: validation says so, with an error.
      await t.click({ role: "tab", name: new RegExp(`^${msg("panel.validation")}`) });
      const validation = await t.find({ role: "tabpanel", name: new RegExp(`^${msg("panel.validation")}`) });
      await until("a Duplicate label error", async () =>
        (await t.text(validation)).includes(msg("validation.E001")),
        30_000,
      );
      await quiet(t);
      await look(t, "duplicate-error");
      await shoot(t, "validation", await around(t, [validation], 0));
      await field(t, msg("field.label"), "Box B");
      await until("the error to go", async () =>
        !(await t.text(validation)).includes(msg("validation.E001")),
        30_000,
      );
      // Align: both boxes' top edges level.
      await t.click({ role: "tab", name: msg("panel.layers") });
      const layers = await t.find({ role: "tree", name: msg("layers.label") });
      await t.click({ role: "treeitem", name: /^Box A/ }, layers);
      const [bx, by] = await t.centre(await t.find({ role: "treeitem", name: /^Box B/ }, layers));
      await t.send("Input.dispatchMouseEvent", { type: "mousePressed", x: bx, y: by, button: "left", buttons: 1, clickCount: 1, modifiers: CTRL });
      await t.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: bx, y: by, button: "left", buttons: 0, clickCount: 1, modifiers: CTRL });
      await status(t, "2 selected", (s) => s.includes(msg("status.selected", { n: 2 })));
      await command(t, msg("command.align.top"));
      // Layers: the picture hidden while checking the seats alone, then shown.
      const picture = await t.find({ role: "treeitem", name: /^Image 1/ }, layers);
      await t.click({ role: "treeitem", name: /^Image 1/ }, layers);
      await chord(t, "h", CTRL);
      await t.find({ role: "button", name: msg("layers.show") }, picture);
      await quiet(t);
      await shoot(t, "arranged", undefined, 0.6);
      await chord(t, "h", CTRL);
      // A guide from the left ruler, by the keyboard.
      const ruler = await t.find({ role: "Canvas", name: msg("ruler.left") });
      await t.call(ruler, "function () { this.focus(); }");
      await t.key("Enter");
      await Bun.sleep(300);
      await look(t, "guide");
      // Snapping off and on.
      const bar = await statusBar(t);
      await t.click({ role: "button", name: msg("status.snap") }, bar);
      await t.find({ role: "button", name: msg("status.snap") }, bar);
      await t.click({ role: "button", name: msg("status.snap") }, bar);
    },
  },
  {
    name: "checking.md: the chart valid",
    async run({ t, look }) {
      await t.click({ role: "tab", name: new RegExp(`^${msg("panel.validation")}`) });
      const panel = await t.find({ role: "tabpanel", name: new RegExp(`^${msg("panel.validation")}`) });
      const text = await until(
        "validation to finish",
        async () => {
          const x = await t.text(panel);
          return x.includes(msg("validation.checking")) ? undefined : x;
        },
        30_000,
      );
      await look(t, "checked");
      // No error; the one warning left is the focal point the chart cannot hold.
      check(
        text.includes(`${msg("validation.errors")} 0`) && text.includes(`${msg("validation.warnings")} 1`) && text.includes("W102"),
        `no errors and only W102 left, not: ${text}`,
      );
    },
  },
  {
    name: "saving.md: Saved; offline, the changes kept; recovery after a reload",
    async run({ t, browser, shoot }) {
      await saveState(t, msg("save.saved"));
      const banner = await t.find({ role: "banner" });
      await shoot(t, "saved", await around(t, [banner], 0).then((c) => ({ ...c, width: 520 })));
      // The connection lost: saving fails, the changes are kept here.
      const offline = async (on: boolean): Promise<void> => {
        await t.send("Network.emulateNetworkConditions", {
          offline: on,
          latency: 0,
          downloadThroughput: -1,
          uploadThroughput: -1,
        });
      };
      const nudge = async (): Promise<void> => {
        await t.click({ role: "tab", name: msg("panel.layers") });
        await t.click({ role: "treeitem", name: /^Box B/ }, await t.find({ role: "tree", name: msg("layers.label") }));
        const c = await canvas(t);
        await t.call(c.id, "function () { this.focus(); }");
        await t.key("ArrowLeft");
      };
      await offline(true);
      await nudge();
      await saveState(t, msg("save.offline"));
      await shoot(t, "offline", await around(t, [banner], 0).then((c) => ({ ...c, width: 620 })));
      await offline(false);
      await saveState(t, msg("save.saved"));
      // Saves refused while the page is reloaded: the change is offered back.
      await t.send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Request" }] });
      const off = browser.on((m) => {
        if (m.sessionId !== t.session) return;
        const p = m.params as Record<string, unknown> | undefined;
        if (m.method === "Fetch.requestPaused" && p !== undefined) {
          const r = p["request"] as { method: string; url: string };
          const save = r.method === "PUT" && r.url.includes("/draft");
          void t.send(save ? "Fetch.failRequest" : "Fetch.continueRequest", {
            requestId: p["requestId"],
            ...(save ? { errorReason: "InternetDisconnected" } : {}),
          });
        }
        if (m.method === "Page.javascriptDialogOpening") {
          void t.send("Page.handleJavaScriptDialog", { accept: true });
        }
      });
      try {
        await nudge();
        await saveState(t, msg("save.offline"));
        const url = await t.evaluate<string>("location.href");
        await t.send("Page.navigate", { url });
        const dialog = await t.find({ role: DIALOG, name: msg("recovery.title") }, undefined, 30_000);
        await shoot(t, "recovery", await around(t, [dialog], 0));
        await t.send("Fetch.disable");
        await t.click({ role: "button", name: msg("recovery.restore") }, dialog);
      } finally {
        off();
        await t.send("Fetch.disable").catch(() => undefined);
      }
      await saveState(t, msg("save.saved"));
    },
  },
  {
    name: "saving.md: the same person in another browser; a colleague opens the chart; taking over",
    async run(g) {
      const { t, browser, owner, shoot, look } = g;
      const url = await t.evaluate<string>("location.href");
      const base = new URL(url).origin;
      const signIn = async (name: string, who: { email: string; password: string }): Promise<Tab> => {
        const tab = await Tab.open(browser, name, 1440, 900);
        tab.allow("GET", /^\/favicon\.ico$/, 404);
        tab.expect("GET", /^\/console\/api\/me$/, 401);
        await tab.goto(`${base}/charts`);
        await tab.enter({ role: "textbox", name: msg("signIn.email") }, who.email);
        await tab.enter({ role: "textbox", name: msg("signIn.password") }, who.password);
        await tab.click({ role: "button", name: msg("signIn.submit") });
        await tab.find({ role: "button", name: msg("user.menu", { email: who.email }) }, undefined, 30_000);
        await tab.find({ role: "link", name: CHART.name }, undefined, 30_000);
        return tab;
      };
      const nudge = async (tab: Tab): Promise<void> => {
        await tab.click({ role: "tab", name: msg("panel.layers") });
        await tab.click({ role: "treeitem", name: /^Box B/ }, await tab.find({ role: "tree", name: msg("layers.label") }));
        const c = await canvas(tab);
        await tab.call(c.id, "function () { this.focus(); }");
        await tab.key("ArrowLeft");
      };
      // The same person in a second browser: not read-only, and whichever saves
      // second is told the chart changed elsewhere (the guide's note).
      const same = await signIn("same-person", owner);
      try {
        await same.goto(url);
        await canvas(same);
        await Bun.sleep(3000);

        const bar = await same.text(await same.find({ role: "banner" }));
        check(!bar.includes(msg("save.readOnly")), `the same person's second browser opens the chart to edit, not: ${bar}`);
        await nudge(same);
        await saveState(same, msg("save.saved"));
        await t.send("Page.bringToFront");
        await nudge(t);
        await saveState(t, msg("save.conflict"));
        await look(t, "conflict");
        // The choice opens by itself; Resolve… in the top bar opens it again.
        const resolve = await t.find({ role: DIALOG, name: msg("conflict.title") });
        await t.click({ role: "button", name: msg("conflict.keepMine") }, resolve);
        await t.gone({ role: DIALOG });
        await saveState(t, msg("save.saved"));
        KNOWN.push("one person in two browsers can edit a chart in both, and the second save is refused");
      } finally {
        await same.close();
        await t.send("Page.bringToFront");
      }
      // Closing the chart in the second browser released the editing lock the
      // two share (one person is one holder, D-73), under this one, which
      // goes on saving without it: a colleague would now open the chart to
      // edit too. Reopening the chart here takes the lock again.
      const lockNow = (): Promise<{ lock: { email: string | null } | null }> =>
        t.evaluate(`(async () => {
          const me = await (await fetch("/console/api/me")).json();
          const r = await fetch("/v1/charts/${CHART.key}/lock", { headers: { "cavea-workspace": me.workspaces[0].id } });
          return r.json();
        })()`);
      check((await lockNow()).lock === null, "closing the second browser no longer frees the lock: update the note in saving.md");
      KNOWN.push("closing a chart in one of a person's two browsers frees the editing lock under the other, which saves on without it");
      await t.goto(url);
      await canvas(t);
      await until("the reopened chart to hold the lock", async () => ((await lockNow()).lock?.email === owner.email ? true : undefined));
      // A colleague: an editor invited from the console's Settings.
      const colleague = { email: "events@example.org", password: `colleague ${Math.random().toString(36).slice(2)}` };
      const inviter = await signIn("inviter", owner);
      let link = "";
      try {
        await inviter.click({ role: "link", name: msg("nav.settings") }, await inviter.find({ role: "navigation", name: msg("nav.label") }));
        await inviter.click({ role: "tab", name: msg("settings.members") });
        await inviter.click({ role: "button", name: msg("invite.new") });
        const form = await inviter.find({ role: DIALOG, name: msg("invite.title") });
        await inviter.enter({ role: "textbox", name: msg("invite.email") }, colleague.email, form);
        await inviter.key("Tab");
        await inviter.type(msg("role.editor").slice(0, 1).toLowerCase());
        await inviter.click({ role: "button", name: msg("invite.create") }, form);
        const shown = await inviter.find({ role: DIALOG, name: msg("invite.shownTitle", { email: colleague.email }) });
        link = await inviter.value(await inviter.find({ role: "textbox", name: msg("invite.link") }, shown));
      } finally {
        await inviter.close();
      }
      const other = await Tab.open(browser, "colleague", 1440, 900);
      other.allow("GET", /^\/favicon\.ico$/, 404);
      other.expect("GET", /^\/console\/api\/me$/, 401);
      try {
        await other.goto(link);
        await other.find({ role: "heading", name: msg("accept.title") });
        await other.enter({ role: "textbox", name: msg("setup.password") }, colleague.password);
        await other.enter({ role: "textbox", name: msg("setup.confirm") }, colleague.password);
        await other.click({ role: "button", name: msg("accept.submit") });
        await other.find({ role: "button", name: msg("user.menu", { email: colleague.email }) }, undefined, 30_000);
        await other.goto(url);
        const banner = await other.find({ role: "banner" }, undefined, 30_000);
        const editing = `Read-only: ${owner.email} is editing`;
        await until("the colleague's designer to open read-only", async () =>
          (await other.text(banner)).includes(editing),
        );
        await quiet(other);
        await shoot(other, "read-only", { ...(await around(other, [banner], 0)), x: 0, width: 1440 }, 0.75);
        await other.click({ role: "button", name: msg("lock.takeOver") }, banner);
        const confirm = await other.find({ role: DIALOG, name: msg("lock.confirmTitle", { who: owner.email }) });
        await other.click({ role: "button", name: msg("lock.takeOverConfirm") }, confirm);
        await other.gone({ role: DIALOG });
        await until(
          "the colleague's designer to be editable",
          async () => ((await other.text(await other.find({ role: "banner" }))).includes("Read-only") ? undefined : true),
          30_000,
        ).catch(async (e: unknown) => {
          await Bun.write(join(g.out, "takeover.fail.txt"), `${other.network.slice(-20).join("\n")}\n${await other.outline()}`);
          throw e;
        });
        // The owner's designer is told, and is read-only now.
        await t.send("Page.bringToFront");
        const told = await t.find({ role: DIALOG, name: msg("lock.lostTitle", { who: colleague.email }) }, undefined, 30_000);
        await shoot(t, "taken-over", await around(t, [told], 0));
        await t.click({ role: "button", name: msg("lock.takeOver") }, told);
        const back = await t.find({ role: DIALOG, name: msg("lock.confirmTitle", { who: colleague.email }) });
        await t.click({ role: "button", name: msg("lock.takeOverConfirm") }, back);
        await t.gone({ role: DIALOG });
        await until(
          "the owner's designer to be editable again",
          async () => ((await t.text(await t.find({ role: "banner" }))).includes("Read-only") ? undefined : true),
          30_000,
        );
      } finally {
        await other.close();
        await t.send("Page.bringToFront");
      }
    },
  },
  {
    name: "publishing.md: preview, publish, the version, an event, a second version",
    async run(g) {
      const { t, shoot } = g;
      const banner = await t.find({ role: "banner" });
      await t.click({ role: "button", name: msg("topbar.preview") }, banner);
      const preview = await t.find({ role: DIALOG, name: msg("preview.title") });
      await t.click({ role: "radio", name: new RegExp(`^${msg("preview.phone")}`) }, preview);
      await t.click({ role: "radio", name: msg("preview.half") }, preview);
      await Bun.sleep(2500);
      await shoot(t, "preview", await around(t, [preview], 0), 0.6);
      await t.click({ role: "button", name: msg("action.close") }, preview);
      await t.gone({ role: DIALOG });
      const publish = async (version: number, seats: number): Promise<void> => {
        await t.click({ role: "button", name: msg("topbar.publish") }, await t.find({ role: "banner" })).catch(async (e: unknown) => {
          g.log(`  publish refused: ${await t.text(await t.find({ role: "banner" }))} | ${await t.text(await statusBar(t))}`);
          throw e;
        });
        const dialog = await t.find({ role: DIALOG, name: msg("publish.title", { name: CHART.name }) });
        await t.click({ role: "button", name: msg("publish.action") }, dialog);
        const done = msg("publish.done", { version, seats: String(seats) });
        await until(`“${done}”`, async () => (await t.text(dialog)).includes(done), 30_000);
        if (version === 1) await shoot(t, "published", await around(t, [dialog], 0));
        g.log(`  publish: ${await t.text(dialog)}`);
        await t.click({ role: "button", name: msg("action.close") }, dialog);
        await t.gone({ role: DIALOG });
      };
      await publish(1, 136);
      // The charts list's picture is made in the background after publishing,
      // by this page: leaving at once would leave the version without one.
      await until("the thumbnail to be attached", async () =>
        t.network.some((n) => /^200 \w+ .*\/versions\/1\/thumbnail/.test(n)) ? true : undefined,
        30_000,
      );
      // The console: the chart's picture in the list, its versions, an event on version 1.
      await t.click({ role: "button", name: msg("menu.label") }, banner);
      await t.click({ role: "menuitem", name: msg("menu.console") });
      const picture = { role: "image", name: msg("charts.thumbnail", { name: CHART.name }) } as const;
      await t.find({ role: "heading", name: msg("charts.title") });
      await Bun.sleep(2000);
      if (!(await t.has(picture))) {
        KNOWN.push("the charts list shows a chart's new picture only once the page is reloaded");
        await t.goto(await t.evaluate<string>("location.href"));
      }
      await t.find(picture, undefined, 30_000);
      await t.click({ role: "link", name: CHART.name });
      await t.find({ role: "tab", name: msg("chart.events") });
      const versions = await t.find({ role: "table", name: msg("chart.versions") });
      await shoot(t, "versions", await around(t, [versions], 8));
      await t.click({ role: "button", name: msg("chart.createEvent") }, versions);
      const form = await t.find({ role: DIALOG, name: msg("events.newFromTitle", { name: CHART.name }) });
      await t.enter({ role: "textbox", name: msg("events.name") }, "Spring Concert", form);
      await t.click({ role: "button", name: msg("events.create") }, form);
      await t.gone({ role: DIALOG });
      // A change and a second version: the event keeps the first.
      await t.click({ role: "link", name: msg("charts.open") });
      await t.find({ role: "application", name: msg("canvas.label", { name: CHART.name }) }, undefined, 30_000);
      await t.click({ role: "tab", name: msg("panel.layers") });
      await t.click({ role: "treeitem", name: /^Box B/ }, await t.find({ role: "tree", name: msg("layers.label") }));
      await field(t, msg("field.rotation"), "-5");
      await saveState(t, msg("save.saved"));
      await publish(2, 136);
      const events = await t.evaluate<Array<{ name: string; version: { number: number } }>>(`(async () => {
        const me = await (await fetch("/console/api/me")).json();
        const r = await fetch("/v1/events", { headers: { "cavea-workspace": me.workspaces[0].id } });
        return (await r.json()).data;
      })()`);
      check(
        events.length === 1 && events[0]?.version.number === 1,
        `the event still on version 1 after version 2: ${JSON.stringify(events)}`,
      );
    },
  },
  {
    name: "start.md: from a template: Open the designer, Theatre, Export chart file, Import chart file",
    async run({ t, shoot, out }) {
      await t.goto(`${new URL(await t.evaluate<string>("location.href")).origin}/charts`);
      await t.click({ role: "button", name: msg("charts.openDesigner") });
      const theatre = await t.find({ role: "button", name: new RegExp(`^${msg("template.theatre")} `) }, undefined, 30_000);
      const empty = await t.find({ role: "button", name: new RegExp(`^${msg("new.empty")}`) });
      const heading = await t.find({ role: "heading", name: msg("new.title") });
      const banquet = await t.find({ role: "button", name: new RegExp(`^${msg("template.banquet")} `) });
      const image = await t.find({ role: "button", name: new RegExp(`^${msg("new.image")}`) });
      await shoot(t, "start", await around(t, [heading, empty, image, theatre, banquet], 16), 0.75);
      await t.click({ role: "button", name: new RegExp(`^${msg("template.theatre")} `) });
      await t.find({ role: "application", name: msg("canvas.label", { name: msg("template.theatre") }) }, undefined, 30_000);
      await t.click({ role: "button", name: msg("tour.skip") }).catch(() => undefined);
      // What the browser saves, as the download would: the file's name and text.
      await t.evaluate(`(() => {
        const click = HTMLAnchorElement.prototype.click;
        const make = URL.createObjectURL;
        let blob = null;
        URL.createObjectURL = (b) => { blob = b; return make.call(URL, b); };
        HTMLAnchorElement.prototype.click = function () {
          if (this.download && blob) blob.text().then((text) => (window.saved = { name: this.download, text }));
          return click.call(this);
        };
      })()`);
      const banner = await t.find({ role: "banner" });
      await t.click({ role: "button", name: msg("menu.label") }, banner);
      await t.click({ role: "menuitem", name: new RegExp(`^${msg("menu.export")}`) });
      const saved = await until("the chart file to be saved", () =>
        t.evaluate<{ name: string; text: string } | null>("window.saved ?? null").then((x) => x ?? undefined),
      );
      check(saved.name.endsWith(".json"), `the exported file ends in .json: ${saved.name}`);
      const file = join(out, saved.name);
      await Bun.write(file, saved.text);
      await t.click({ role: "button", name: msg("menu.label") }, banner);
      await t.click({ role: "menuitem", name: msg("menu.console") });
      await t.find({ role: "heading", name: msg("charts.title") });
      await t.send("Page.setInterceptFileChooserDialog", { enabled: true });
      await t.click({ role: "button", name: msg("charts.import") });
      await pick(t, 'input[type=file][accept=".json,application/json"]', file);
      await t.send("Page.setInterceptFileChooserDialog", { enabled: false });
      const form = await t.find({ role: DIALOG, name: msg("charts.importTitle") });
      await t.enter({ role: "textbox", name: msg("charts.name") }, "Studio theatre", form);
      await t.click({ role: "button", name: msg("charts.create") }, form);
      await t.gone({ role: DIALOG });
      await t.find({ role: "link", name: "Studio theatre" });
    },
  },
];

