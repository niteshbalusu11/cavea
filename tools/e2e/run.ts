// The end-to-end check (docs/13-testing-benchmarks.md §9, TST-031): the
// product's whole first path, in one run, against the production build: the
// server binary with the console compiled in, under its own Content Security
// Policy, on a database of its own, driven in the flake's headless Chromium
// as people drive it. Run by `cargo xtask e2e`, which builds what it needs:
//
//   bun tools/e2e/run.ts --server <cavea-server> [--keep] [--headed] [--to <step>]
//
// Each step ends in checks a machine evaluates. A failure names the step,
// what was expected and what was found, and leaves a screenshot, the
// accessibility tree, the console and the network log of every open page
// under target/e2e/<run>/. A step marked `known` (a product bug written up,
// not yet fixed) is reported as a known failure and the run goes on. Exit
// codes: 0 every step passed, 1 a step failed (or a known failure passed),
// 2 usage, 3 the check could not run (no PostgreSQL, no browser, the server
// did not start), 4 every step passed but known failures.
import { SQL } from "bun";
import { randomBytes } from "node:crypto";
import { chmod, copyFile, mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Browser, pinned } from "../chromium.ts";
import { freePort, Server } from "./server.ts";
import { STEPS, type Run } from "./steps.ts";
import type { Tab } from "./tab.ts";

const ROOT = join(import.meta.dir, "../..");
const PG = process.env["CAVEA_E2E_POSTGRES"] ?? "postgres://cavea:cavea@127.0.0.1:5432";

const args = process.argv.slice(2);
const flag = (name: string): string | undefined => {
  const at = args.indexOf(name);
  return at === -1 ? undefined : args[at + 1];
};
const keep = args.includes("--keep");
const headed = args.includes("--headed");
const serverBinary = flag("--server") ?? join(ROOT, "target/release/cavea-server");
const to = flag("--to");
const known = new Set(args.filter((a, i) => !a.startsWith("--") && !args[i - 1]?.startsWith("--")));
if (known.size > 0 || (to !== undefined && !/^\d+$/.test(to))) {
  console.error(
    "usage: bun tools/e2e/run.ts [--server <cavea-server>] [--keep] [--headed] [--to <step number>]",
  );
  process.exit(2);
}

const hex = (bytes: number): string => randomBytes(bytes).toString("hex");
const id = `${new Date().toISOString().replace(/[-:]/g, "").slice(0, 15)}-${process.pid}`;
const OUT = join(ROOT, "target/e2e", id);
const started = performance.now();
const seconds = (from: number): string => ((performance.now() - from) / 1000).toFixed(1);

// ------------------------------------------------------------- clean-up

const cleanup: Array<{ what: string; run: () => Promise<void> }> = [];
let tidying: Promise<void> | undefined;
/** Runs every clean-up, newest first, once; a failing one is reported and the rest still run. */
function tidy(): Promise<void> {
  tidying ??= (async () => {
    while (cleanup.length > 0) {
      const c = cleanup.pop();
      await c
        ?.run()
        .catch((error: unknown) => console.error(`clean-up (${c.what}): ${String(error)}`));
    }
  })();
  return tidying;
}
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const) {
  process.on(signal, () => {
    console.error(`\n${signal}: stopping everything this run started`);
    void tidy().then(() => process.exit(130));
  });
}

// ---------------------------------------------------------------- server

/** The buyer's page on its own origin (`127.0.0.1`, the server is `localhost`), under the policy 05 §10 gives such a page. */
function buyerSite(serverPort: number): { origin: string; stop: () => void } {
  const dist = join(ROOT, "packages/renderer/dist");
  const page = join(import.meta.dir, "buyer");
  const policy = [
    "default-src 'none'",
    "script-src 'self' 'wasm-unsafe-eval'",
    `connect-src 'self' http://localhost:${serverPort} ws://localhost:${serverPort}`,
    "require-trusted-types-for 'script'",
  ].join("; ");
  const site = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      const path = decodeURIComponent(new URL(request.url).pathname);
      if (path === "/favicon.ico") {
        return new Response(null, { status: 204 });
      }
      const file = path.includes("..")
        ? undefined
        : path.startsWith("/pkg/")
          ? Bun.file(join(dist, path.slice(5)))
          : Bun.file(join(page, path === "/" ? "index.html" : path));
      const headers = { "content-security-policy": policy };
      if (file === undefined || !(await file.exists())) {
        return new Response("not found", { status: 404, headers });
      }
      return new Response(file, { headers });
    },
  });
  return { origin: `http://127.0.0.1:${site.port}`, stop: () => void site.stop(true) };
}

// ------------------------------------------------------------------ main

interface Outcome {
  step: string;
  result: "pass" | "FAIL" | "known failure";
  seconds: number;
}

async function report(run: Run, prefix: string): Promise<string[]> {
  const written: string[] = [];
  for (const tab of run.tabs.values()) {
    const base = join(OUT, `${prefix}-${tab.name}`);
    try {
      await writeFile(`${base}.png`, await tab.screenshot());
      written.push(`${base}.png`);
    } catch {
      // A closed or crashed page has no picture; its logs still help.
    }
    await writeFile(
      `${base}.tree.txt`,
      `${await tab.outline().catch((e: unknown) => String(e))}\n`,
    );
    await writeFile(`${base}.console.txt`, `${tab.console.join("\n")}\n`);
    await writeFile(`${base}.network.txt`, `${tab.network.join("\n")}\n`);
  }
  return written;
}

async function main(): Promise<number> {
  await mkdir(OUT, { recursive: true });
  // The binary is copied, so a build in this tree during the run cannot
  // change what the restart starts.
  const binary = join(OUT, "cavea-server");
  await copyFile(serverBinary, binary).catch((error: unknown) => {
    throw new Error(
      `no server binary at ${serverBinary} (run \`cargo xtask e2e\`): ${String(error)}`,
    );
  });
  await chmod(binary, 0o755);

  // A database of this run's own on the shared PostgreSQL, dropped on every exit path.
  const database = `cavea_e2e_${process.pid}_${hex(3)}`;
  const admin = new SQL(`${PG}/cavea`, { max: 1 });
  try {
    await admin.unsafe(`CREATE DATABASE ${database}`);
  } catch (error) {
    await admin.close();
    throw new Error(
      `cannot create a database on ${PG} (start it with \`docker compose -f deploy/compose.yaml up -d --wait\`): ${String(error)}`,
      { cause: error },
    );
  }
  cleanup.push({
    what: `drop database ${database}`,
    run: async () => {
      await admin.unsafe(`DROP DATABASE IF EXISTS ${database} WITH (FORCE)`);
      await admin.close();
    },
  });

  const port = freePort();
  const base = `http://localhost:${port}`;
  const adminKey = hex(24);
  const server = new Server(
    binary,
    {
      CAVEA_DATABASE_URL: `${PG}/${database}`,
      CAVEA_LISTEN: `127.0.0.1:${port}`,
      CAVEA_ADMIN_KEY: adminKey,
      CAVEA_PUBLIC_URL: base,
      TMPDIR: join(OUT, "tmp"),
    },
    base,
    OUT,
  );
  await mkdir(join(OUT, "tmp"), { recursive: true });
  cleanup.push({ what: "stop cavea-server", run: async () => void (await server.stop()) });
  await server.start();

  const buyer = buyerSite(port);
  cleanup.push({ what: "stop the buyer's page", run: async () => buyer.stop() });

  const { chromium, fonts } = await pinned(ROOT);
  const browser = await Browser.launch({
    chromium,
    fonts,
    name: "cavea-e2e",
    profile: join(OUT, "profile"),
    headed,
    // `localhost` is the server's name and must reach its IPv4 listener.
    args: ["--host-resolver-rules=MAP localhost 127.0.0.1"],
  });
  cleanup.push({
    what: "close Chromium",
    run: async () => {
      await browser.close();
      await rm(join(OUT, "profile"), { recursive: true, force: true });
    },
  });

  const run: Run = {
    root: ROOT,
    out: OUT,
    base,
    buyer: buyer.origin,
    browser,
    server,
    tabs: new Map<string, Tab>(),
    secrets: new Map<string, string>([["admin key", adminKey]]),
    notes: [],
    latency: [],
  };
  console.log(
    `e2e ${id}: ${browser.version}, headless ${!headed}, SwiftShader; server ${base}; buyer ${buyer.origin}; database ${database}`,
  );
  console.log(`output: ${OUT}\n`);

  const outcomes: Outcome[] = [];
  const last = to === undefined ? STEPS.length : Number(to);
  let failed = false;
  let knownFailures = 0;
  for (const [i, step] of STEPS.entries()) {
    if (i + 1 > last) break;
    const label = `${i + 1}. ${step.name}`;
    const from = performance.now();
    process.stdout.write(`${label} … `);
    try {
      await step.run(run);
      // Whatever the pages did besides: each must have nothing to show.
      const problems: string[] = [];
      for (const tab of run.tabs.values()) {
        await tab.collect();
        problems.push(...tab.problems.splice(0).map((p) => `${tab.name}: ${p.kind}: ${p.text}`));
      }
      if (problems.length > 0) {
        throw new Error(`the pages reported problems no step expects:\n  ${problems.join("\n  ")}`);
      }
      if (step.known !== undefined) {
        // A known failure that passes is news: the mark must go.
        outcomes.push({ step: label, result: "FAIL", seconds: Number(seconds(from)) });
        console.log(`passes, but is marked as a known failure (${step.known}): remove the mark`);
        failed = true;
        break;
      }
      outcomes.push({ step: label, result: "pass", seconds: Number(seconds(from)) });
      console.log(`ok (${seconds(from)} s)`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const pictures = await report(run, `step${i + 1}`);
      const marked = step.known !== undefined;
      outcomes.push({
        step: label,
        result: marked ? "known failure" : "FAIL",
        seconds: Number(seconds(from)),
      });
      console.log(
        `${marked ? `KNOWN FAILURE (${step.known})` : "FAILED"} (${seconds(from)} s)\n\n${message}\n`,
      );
      if (pictures.length > 0) {
        console.log(
          `screenshots, accessibility trees, console and network logs: ${OUT}/step${i + 1}-*`,
        );
      }
      console.log(`server log: ${OUT}/server.log\n`);
      // Its pages' problems are part of the failure, not the next step's.
      for (const tab of run.tabs.values()) tab.problems.splice(0);
      if (marked) {
        knownFailures++;
        continue;
      }
      failed = true;
      break;
    }
  }

  console.log("\n| Step | Result | Seconds |\n|---|---|---|");
  for (const o of outcomes) {
    console.log(`| ${o.step} | ${o.result} | ${o.seconds.toFixed(1)} |`);
  }
  for (const note of run.notes) {
    console.log(`\n${note}`);
  }
  const verdict = failed
    ? "FAILED"
    : knownFailures > 0
      ? `passed but for ${knownFailures} known failure${knownFailures === 1 ? "" : "s"}`
      : "passed";
  console.log(`\n${verdict} in ${seconds(started)} s`);

  if (keep) {
    const file = join(OUT, "keep.txt");
    await writeFile(
      file,
      `console: ${base}/charts\nbuyer: ${run.buyer}\n${[...run.secrets].map(([k, v]) => `${k}: ${v}`).join("\n")}\n`,
      { mode: 0o600 },
    );
    console.log(
      `\n--keep: the server stays up at ${base}; sign-in details in ${file}. Ctrl+C stops everything.`,
    );
    await new Promise(() => undefined);
  }
  return failed ? 1 : knownFailures > 0 ? 4 : 0;
}

let code = 3;
try {
  code = await main();
} catch (error) {
  console.error(
    `\nthe check could not run: ${error instanceof Error ? error.message : String(error)}`,
  );
  code = 3;
} finally {
  await tidy();
}
process.exit(code);
