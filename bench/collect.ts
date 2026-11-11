// The benchmark collector (docs/13-testing-benchmarks.md §6.2): receives the
// benchmark page's result (apps/demo/bench.html) from a device on the local
// network and writes it to a file. Every byte it receives is hostile: the body
// is at most 1 MiB and must be one `BenchResult` exactly (every key known,
// every value its type); the file name comes from the clock and a counter,
// never from the request; nothing it receives is printed.
//
//   bun bench/collect.ts [--listen host:port] [--out dir]
//
// Default `127.0.0.1:8787` (loopback only: give the machine's address to take
// results from a phone) and `bench/local-results/` (gitignored: results from
// development machines are not reference figures; those go to bench/results/
// with the commit, TST-016).
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { problem } from "../apps/demo/src/bench-result.ts";

const MAX_BYTES = 1 << 20;

const args = process.argv.slice(2);
const option = (name: string, fallback: string): string => {
  const at = args.indexOf(name);
  return at === -1 ? fallback : (args[at + 1] ?? fallback);
};
const listen = option("--listen", "127.0.0.1:8787");
const out = option("--out", join(import.meta.dir, "local-results"));
const match = listen.match(/^(.+):(\d+)$/);
if (match === null) {
  console.error("usage: bun bench/collect.ts [--listen host:port] [--out dir]");
  process.exit(2);
}
await mkdir(out, { recursive: true });

// The page is served from another origin (the demo's dev server): it may post
// JSON here from anywhere it is loaded, and read only the status.
const cors = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "POST",
  "access-control-allow-headers": "content-type",
  "access-control-max-age": "600",
};
const answer = (status: number, text: string): Response =>
  new Response(text, { status, headers: { ...cors, "content-type": "text/plain" } });

let saved = 0;
const server = Bun.serve({
  hostname: match[1] ?? "127.0.0.1",
  port: Number(match[2]),
  maxRequestBodySize: MAX_BYTES,
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path !== "/results") {
      return answer(404, "not found");
    }
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }
    if (request.method !== "POST") {
      return answer(405, "POST only");
    }
    if (request.headers.get("content-type")?.split(";")[0]?.trim() !== "application/json") {
      return answer(415, "application/json only");
    }
    const declared = Number(request.headers.get("content-length") ?? 0);
    if (declared > MAX_BYTES) {
      return answer(413, "too large");
    }
    let body: ArrayBuffer;
    try {
      body = await request.arrayBuffer();
    } catch {
      return answer(400, "unreadable body");
    }
    if (body.byteLength > MAX_BYTES) {
      return answer(413, "too large");
    }
    let value: unknown;
    try {
      value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(body));
    } catch {
      return answer(400, "not JSON");
    }
    const wrong = problem(value);
    if (wrong !== undefined) {
      console.log(`refused a result: ${wrong}`);
      return answer(400, wrong);
    }
    saved++;
    const name = `${new Date().toISOString().replaceAll(":", "-")}-${saved}.json`;
    // Written as parsed and checked, not as received.
    await writeFile(join(out, name), `${JSON.stringify(value, null, 2)}\n`, { flag: "wx" });
    console.log(`saved ${name} (${body.byteLength} B)`);
    return answer(201, name);
  },
});
console.log(`collecting at http://${server.hostname}:${server.port}/results into ${out}`);
