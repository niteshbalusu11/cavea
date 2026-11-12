// The renderer's golden images (docs/13-testing-benchmarks.md §6.1). Run by
// `cargo xtask golden`, which first builds the release WASM and the CLI:
//
//   bun tools/golden/run.ts [--update] [--runs n] [--cli path] [case…]
//
// Compiles the charts that `crates/cavea-render/golden/cases.json` names with
// the CLI, renders every case in the flake's Chromium (page.ts) and compares
// it with the stored PNG beside the cases. A missing baseline fails; only
// `--update` writes baselines (the named cases, or all), together with the
// Chromium version they were drawn by. With `--runs n` every case is drawn in
// n separate browser starts and the runs must match byte for byte. On a
// failure the drawn image and a difference image go to `target/golden/`.
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Browser, pinned, sh } from "../chromium.ts";
import { decode, encode, type Image } from "./png.ts";
import type { Panel, Rendered } from "./page.ts";

const ROOT = join(import.meta.dir, "../..");
const GOLDEN = join(ROOT, "crates/cavea-render/golden");
const OUT = join(ROOT, "target/golden");
/** Beside the baselines: the browser that drew them. */
const VERSION_FILE = join(GOLDEN, "chromium.txt");

/**
 * A pixel differs when any channel is more than this many levels from the
 * baseline's. SwiftShader is exact run to run on one machine; this absorbs a
 * level of rounding in anti-aliased edges if another CPU's code generation
 * rounds a shader's arithmetic differently, and nothing a person would see.
 */
const CHANNEL_TOLERANCE = 2;
/**
 * A panel fails when more than this share of its pixels differ: 0.02%, 15
 * pixels of a 320 × 240 panel. Small enough that one seat label of two
 * digits going missing (about 40 pixels at the detail level) fails.
 */
const MAX_DIFFERENT_SHARE = 0.0002;

interface ChartSpec {
  preset: string;
  seed?: number;
  /** Replaces `settings.seatShape`. */
  seatShape?: "circle" | "rounded" | "chair";
  /** Replaces the section labels, in document order. */
  sections?: string[];
  /** Replaces every text object's text. */
  text?: string;
  /** Row label prefix for the first section's rows. */
  rowPrefix?: string;
  /** Seat label scheme for the second section's rows. */
  seatScheme?: string;
  /**
   * A background image node added to the document: the file beside the
   * cases, served under `hash` (any 32 lowercase hex digits), with the
   * image node's own fields (`origin`, `size`, `rotation`, `opacity`).
   */
  image?: { file: string; hash: string; node: Record<string, unknown> };
}

interface Case {
  name: string;
  /** What a failure of this case means. */
  guards: string;
  panels: Panel[];
}

interface Cases {
  charts: Record<string, ChartSpec>;
  cases: Case[];
}

const args = process.argv.slice(2);
const flag = (name: string): string | undefined => {
  const at = args.indexOf(name);
  return at === -1 ? undefined : args.splice(at, 2)[1];
};
const update = args.includes("--update");
const runs = Number(flag("--runs") ?? 1);
const cli = flag("--cli") ?? join(ROOT, "target/release/cavea");
const only = args.filter((a) => !a.startsWith("--"));

const cases = JSON.parse(await readFile(join(GOLDEN, "cases.json"), "utf8")) as Cases;
const chosen = cases.cases.filter((c) => only.length === 0 || only.includes(c.name));
if (chosen.length === 0 || chosen.length < only.length) {
  console.error(`unknown case; cases: ${cases.cases.map((c) => c.name).join(", ")}`);
  process.exit(2);
}

const started = performance.now();
const work = join(OUT, "serve");
await rm(OUT, { recursive: true, force: true });
await mkdir(join(work, "charts"), { recursive: true });

// Charts: a preset's document, changed as the case file says, compiled.
type Doc = {
  nextOid: number;
  settings: Record<string, unknown>;
  nodes: Array<Record<string, unknown> & { type: string }>;
};
const used = new Set(chosen.flatMap((c) => c.panels.map((p) => p.chart)));
/** Background images by served path. */
const imageFiles: Record<string, string> = {};
for (const name of used) {
  const spec = cases.charts[name];
  if (spec === undefined) {
    throw new Error(`chart ${name} is not in cases.json`);
  }
  const doc = JSON.parse(
    await sh([cli, "gen", spec.preset, "--seed", String(spec.seed ?? 1)]),
  ) as Doc;
  if (spec.seatShape !== undefined) {
    doc.settings["seatShape"] = spec.seatShape;
  }
  const sections = doc.nodes.filter((n) => n.type === "section");
  sections.forEach((section, i) => {
    const label = spec.sections?.[i];
    if (label !== undefined) {
      section["label"] = label;
    }
    const blocks = (section["children"] as Array<Record<string, Record<string, unknown>>>).filter(
      (c) => (c["type"] as unknown) === "rowBlock",
    );
    for (const block of blocks) {
      if (i === 0 && spec.rowPrefix !== undefined) {
        (block["rowLabels"] as Record<string, unknown>)["prefix"] = spec.rowPrefix;
      }
      if (i === 1 && spec.seatScheme !== undefined) {
        const labels = block["seatLabels"] as Record<string, unknown>;
        labels["scheme"] = spec.seatScheme;
        // A numeric start (101) would make the letters run from "cw".
        delete labels["start"];
      }
    }
  });
  for (const text of doc.nodes.filter((n) => n.type === "text")) {
    if (spec.text !== undefined) {
      text["text"] = spec.text;
    }
  }
  if (spec.image !== undefined) {
    // First, so it is drawn under everything (it is anyway: pass 2).
    doc.nodes.unshift({
      ...spec.image.node,
      id: doc.nextOid++,
      type: "image",
      asset: spec.image.hash,
      role: "background",
    });
    imageFiles[`/assets/${spec.image.hash}`] = join(GOLDEN, spec.image.file);
  }
  const docPath = join(work, `${name}.json`);
  await writeFile(docPath, JSON.stringify(doc));
  const dir = join(work, `compiled-${name}`);
  await sh([cli, "compile", docPath, "-o", dir]);
  const ccf = (await Array.fromAsync(new Bun.Glob("*.ccf").scan(dir))).find(
    (f) => !f.endsWith(".full.ccf"),
  );
  if (ccf === undefined) {
    throw new Error(`no public artefact for ${name}`);
  }
  await Bun.write(join(work, "charts", `${name}.ccf`), Bun.file(join(dir, ccf)));
}

// The page: page.ts bundled, the WASM and the atlas as the SDK ships them.
const built = await Bun.build({
  entrypoints: [join(import.meta.dir, "page.ts")],
  target: "browser",
  format: "esm",
});
if (!built.success || built.outputs[0] === undefined) {
  throw new Error(`bundling the page failed: ${built.logs.join("\n")}`);
}
const script = await built.outputs[0].text();
const files: Record<string, string> = {
  "/wasm/cavea_web_bg.wasm": join(ROOT, "packages/renderer/wasm/cavea_web_bg.wasm"),
  "/glyphs.png": join(ROOT, "packages/renderer/assets/glyphs.png"),
  ...imageFiles,
};
const server = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/") {
      return new Response(
        `<!doctype html><meta charset="utf-8"><body style="margin:0"></body><script type="module" src="page.js"></script>`,
        { headers: { "content-type": "text/html" } },
      );
    }
    if (path === "/page.js") {
      return new Response(script, { headers: { "content-type": "text/javascript" } });
    }
    const chart = path.match(/^\/charts\/([a-z0-9-]+)\.ccf$/)?.[1];
    const file = chart === undefined ? files[path] : join(work, "charts", `${chart}.ccf`);
    return file === undefined
      ? new Response("not found", { status: 404 })
      : new Response(Bun.file(file));
  },
});

/** Draws every panel of every chosen case in one browser start; images by case name. */
async function drawAll(): Promise<{ version: string; images: Map<string, Image[]> }> {
  const { chromium, fonts } = await pinned(ROOT);
  const browser = await Browser.launch({
    chromium,
    fonts,
    name: "cavea-golden",
    args: ["--disable-lcd-text", "--force-color-profile=srgb"],
  });
  try {
    const page = await browser.open(`http://127.0.0.1:${server.port}/`);
    await page.waitFor("window.render");
    const images = new Map<string, Image[]>();
    for (const c of chosen) {
      const panels: Image[] = [];
      for (const panel of c.panels) {
        const r = await page
          .evaluate<Rendered>(`render(${JSON.stringify(panel)})`)
          .catch((error: unknown) => {
            throw new Error(`${c.name}: ${String(error)}`);
          });
        panels.push({
          width: r.width,
          height: r.height,
          rgba: new Uint8Array(Buffer.from(r.rgba, "base64")),
        });
      }
      images.set(c.name, panels);
    }
    const errors = browser.errors();
    if (errors.length > 0) {
      throw new Error(`page errors: ${errors.join("\n")}`);
    }
    await page.close();
    return { version: browser.version, images };
  } finally {
    await browser.close();
  }
}

/** The panels side by side, top-aligned, on black. */
function tile(panels: Image[]): Image {
  const width = panels.reduce((w, p) => w + p.width, 0);
  const height = Math.max(...panels.map((p) => p.height));
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 3; i < rgba.length; i += 4) {
    rgba[i] = 255;
  }
  let left = 0;
  for (const p of panels) {
    for (let y = 0; y < p.height; y++) {
      rgba.set(p.rgba.subarray(y * p.width * 4, (y + 1) * p.width * 4), (y * width + left) * 4);
    }
    left += p.width;
  }
  return { width, height, rgba };
}

/** Pixels over the tolerance, per panel (by x range), and a difference image. */
function compare(
  actual: Image,
  baseline: Image,
  panels: Image[],
): { counts: number[]; diff: Image; worst: number } {
  const counts = panels.map(() => 0);
  const diff = new Uint8Array(actual.rgba.length);
  let worst = 0;
  for (let y = 0; y < actual.height; y++) {
    for (let x = 0; x < actual.width; x++) {
      const i = (y * actual.width + x) * 4;
      let d = 0;
      for (let k = 0; k < 3; k++) {
        d = Math.max(d, Math.abs((actual.rgba[i + k] ?? 0) - (baseline.rgba[i + k] ?? 0)));
      }
      worst = Math.max(worst, d);
      // Differing pixels red; the rest the drawn image, faded.
      const grey =
        ((actual.rgba[i] ?? 0) + (actual.rgba[i + 1] ?? 0) + (actual.rgba[i + 2] ?? 0)) / 12 + 160;
      diff.set(d > CHANNEL_TOLERANCE ? [255, 0, 0, 255] : [grey, grey, grey, 255], i);
      if (d > CHANNEL_TOLERANCE) {
        let left = 0;
        const panel = panels.findIndex((p) => (left += p.width) > x);
        counts[panel] = (counts[panel] ?? 0) + 1;
      }
    }
  }
  return { counts, diff: { width: actual.width, height: actual.height, rgba: diff }, worst };
}

let failed = 0;
try {
  const draws: Array<Awaited<ReturnType<typeof drawAll>>> = [];
  for (let run = 0; run < runs; run++) {
    draws.push(await drawAll());
  }
  const [first] = draws;
  if (first === undefined) {
    throw new Error("--runs must be at least 1");
  }
  for (const [run, draw] of draws.entries()) {
    for (const [name, panels] of draw.images) {
      const other = first.images.get(name) ?? [];
      const same = panels.every((p, i) =>
        Buffer.from(p.rgba).equals(Buffer.from(other[i]?.rgba ?? [])),
      );
      if (!same) {
        console.log(`FAIL ${name}: browser start ${run + 1} drew different pixels from start 1`);
        failed++;
      }
    }
  }
  if (runs > 1 && failed === 0) {
    console.log(
      `determinism: ${runs} browser starts drew identical pixels for ${chosen.length} cases`,
    );
  }

  const recorded = await readFile(VERSION_FILE, "utf8").then(
    (t) => t.trim(),
    () => "",
  );
  if (update && recorded !== first.version && chosen.length < cases.cases.length) {
    // One version file covers every baseline: a new browser redraws them all.
    throw new Error(
      `the baselines were drawn by ${recorded || "no recorded browser"}, this is ${first.version}: ` +
        "update every case (`cargo xtask golden --update` with no case named), not some",
    );
  }
  if (update) {
    await writeFile(VERSION_FILE, `${first.version}\n`);
  } else if (recorded !== first.version) {
    console.log(
      `FAIL the baselines were drawn by ${recorded || "no recorded browser"}, this is ${first.version}: ` +
        "regenerate them with `cargo xtask golden --update` and review the image diff in the pull request",
    );
    failed++;
  }
  await mkdir(join(OUT, "actual"), { recursive: true });
  let bytes = 0;
  for (const c of chosen) {
    const panels = first.images.get(c.name) ?? [];
    const actual = tile(panels);
    const file = join(GOLDEN, `${c.name}.png`);
    if (update) {
      const png = encode(actual);
      await writeFile(file, png);
      bytes += png.length;
      console.log(`wrote ${c.name}.png: ${actual.width} × ${actual.height}, ${png.length} B`);
      continue;
    }
    const stored = await readFile(file).then(
      (b) => decode(new Uint8Array(b)),
      () => undefined,
    );
    const keep = async (): Promise<void> => {
      await writeFile(join(OUT, "actual", `${c.name}.png`), encode(actual));
    };
    if (stored === undefined) {
      console.log(`FAIL ${c.name}: no baseline ${file}; create it with --update (${c.guards})`);
      await keep();
      failed++;
      continue;
    }
    if (stored.width !== actual.width || stored.height !== actual.height) {
      console.log(
        `FAIL ${c.name}: drawn ${actual.width} × ${actual.height}, baseline ${stored.width} × ${stored.height}`,
      );
      await keep();
      failed++;
      continue;
    }
    const { counts, diff, worst } = compare(actual, stored, panels);
    const over = counts
      .map((n, i) => ({ n, i, share: n / ((panels[i]?.width ?? 1) * (panels[i]?.height ?? 1)) }))
      .filter((p) => p.share > MAX_DIFFERENT_SHARE);
    if (over.length === 0) {
      const total = counts.reduce((a, b) => a + b, 0);
      console.log(`ok   ${c.name}${total > 0 ? ` (${total} pixels within tolerance)` : ""}`);
      continue;
    }
    failed++;
    await keep();
    await writeFile(join(OUT, "actual", `${c.name}.diff.png`), encode(diff));
    for (const p of over) {
      console.log(
        `FAIL ${c.name} panel ${p.i + 1}: ${p.n} pixels (${(p.share * 100).toFixed(3)}%) differ by more than ` +
          `${CHANNEL_TOLERANCE} levels, the largest by ${worst}; guards: ${c.guards}`,
      );
    }
  }
  if (update) {
    console.log(`baselines: ${bytes} B for ${chosen.length} cases; drawn by ${first.version}`);
  }
  if (failed > 0 && !update) {
    console.log(`drawn images and differences: ${join(OUT, "actual")}`);
  }
  const seconds = ((performance.now() - started) / 1000).toFixed(1);
  console.log(
    `${chosen.length} cases, ${failed} failed, ${seconds} s (${first.version}, SwiftShader)`,
  );
} finally {
  server.stop(true);
}
process.exit(failed > 0 ? 1 : 0);
