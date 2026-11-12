// Makes the manager's frame fixtures from a running server (its own
// encoder and its own label index), for `frames.test.ts`:
//
//   CAVEA_ADMIN_KEY=<the server's> bun apps/console/src/console/manager/fixtures/make.ts \
//     --url http://127.0.0.1:<port> --cli target/release/cavea
//
// For two charts (the theatre preset, and the compiler's sample using every
// option, `crates/cavea-compiler/samples/schema-1/full.json`) it creates a
// workspace, publishes the chart, opens an event, gives every seat a status
// by a pattern over `idx` (free, held, booked, blocked, custom), sending
// the labels `chartLabels` reads from the compiled chart, then saves the
// chart, the staff SNAPSHOT, a DELTA after more changes, and the statuses
// by label. A label read wrongly is refused by the server or lands on
// another `idx`: the script checks the snapshot against the pattern and
// fails before writing anything.
import { writeFileSync } from "node:fs";
import { decodeFrame } from "../frames.ts";
import { chartLabels } from "../labels.ts";

const arg = (name: string): string | undefined => {
  const i = process.argv.indexOf(name);
  return i < 0 ? undefined : process.argv[i + 1];
};
const base = arg("--url") ?? "";
const cli = arg("--cli") ?? "target/release/cavea";
const admin = process.env["CAVEA_ADMIN_KEY"] ?? "";
if (base === "" || admin === "") {
  console.error("usage: CAVEA_ADMIN_KEY=… bun make.ts --url http://127.0.0.1:<port> [--cli path]");
  process.exit(2);
}
const out = new URL(".", import.meta.url).pathname;
const run = Date.now().toString(36);

async function api(method: string, path: string, key: string, body?: unknown): Promise<unknown> {
  const response = await fetch(base + path, {
    method,
    headers: {
      authorization: `Bearer ${key}`,
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const text = await response.text();
  if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${text}`);
  return text === "" ? null : JSON.parse(text);
}

async function state(key: string, event: string, after: number): Promise<Uint8Array | null> {
  const response = await fetch(`${base}/v1/events/${event}/state?after_seq=${after}`, {
    headers: { authorization: `Bearer ${key}` },
  });
  if (response.status === 204) return null;
  if (response.status !== 200) throw new Error(`state: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}

/** The state after `after`, once the publication has caught up with `seq`. */
async function stateAt(key: string, event: string, after: number, seq: number) {
  for (let i = 0; i < 100; i++) {
    const bytes = await state(key, event, after);
    if (bytes !== null) {
      const frame = decodeFrame(bytes);
      const at = frame.type === "snapshot" ? frame.seq : frame.type === "delta" ? frame.to : -1;
      if (at >= seq) return { bytes, frame };
    }
    await Bun.sleep(50);
  }
  throw new Error("the state never reached the committed seq");
}

const chunks = <T>(list: T[], n: number): T[][] =>
  Array.from({ length: Math.ceil(list.length / n) }, (_, i) => list.slice(i * n, i * n + n));

/** Gives `labels` status `code` (0 to 15) with the commands an operator and
 * a buyer would use. Returns the last `seq`. */
async function give(key: string, event: string, labels: string[], code: number): Promise<number> {
  let seq = 0;
  for (const part of chunks(labels, 100)) {
    let reply: { seq: number };
    if (code === 1) {
      const token = (await api("POST", "/v1/hold-tokens", key, {
        event,
        max_objects: 100,
        ttl_seconds: 7200,
      })) as { hold_token: string };
      reply = (await api("POST", `/v1/events/${event}/hold`, key, {
        hold_token: token.hold_token,
        objects: part,
      })) as { seq: number };
    } else if (code === 2) {
      reply = (await api("POST", `/v1/events/${event}/book`, key, {
        objects: part,
        order_ref: `fixture-${run}`,
      })) as { seq: number };
    } else {
      reply = (await api("POST", `/v1/events/${event}/change-status`, key, {
        objects: part,
        status: code === 3 ? "blocked" : code === 0 ? "free" : code,
      })) as { seq: number };
    }
    seq = Math.max(seq, reply.seq);
  }
  return seq;
}

function shell(command: string[]): string {
  const p = Bun.spawnSync(command, { stderr: "inherit" });
  if (!p.success) throw new Error(`${command.join(" ")} failed`);
  return p.stdout.toString();
}

async function make(name: string, document: unknown, pattern: (idx: number) => number) {
  const workspace = (await api("POST", "/v1/workspaces", admin, {
    key: `fx-${name}-${run}`,
    name: `Fixture ${name}`,
  })) as { id: string };
  const minted = (await api("POST", `/v1/workspaces/${workspace.id}/keys`, admin, {
    name: "fixtures",
  })) as { key: string };
  const key = minted.key;
  await api("POST", "/v1/charts", key, { key: name, name, document });
  await api("POST", `/v1/charts/${name}/publish`, key);
  const event = (await api("POST", "/v1/events", key, { key: `${name}-night`, chart: name })) as {
    key: string;
    version: { url: string };
  };
  const chart = new Uint8Array(await (await fetch(base + event.version.url)).arrayBuffer());
  const labels = chartLabels(chart);

  const want = labels.map((_, i) => pattern(i));
  let seq = 0;
  for (let code = 1; code <= 15; code++) {
    const these = labels.filter((_, i) => want[i] === code);
    if (these.length > 0) seq = Math.max(seq, await give(key, event.key, these, code));
  }
  const first = await stateAt(key, event.key, 0, seq);
  if (first.frame.type !== "snapshot") throw new Error("expected a snapshot");
  const snap = first.frame;
  if (snap.codes.length !== labels.length) throw new Error("object counts differ");
  snap.codes.forEach((c, i) => {
    if (c !== want[i]) throw new Error(`${labels[i]} (idx ${i}) is ${c}, not ${want[i]}`);
  });

  // Then: release some bookings, unblock some seats, book some free ones.
  const now = [...want];
  const pick = (code: number, every: number) =>
    labels.filter((_, i) => want[i] === code && i % every === 0);
  const released = pick(2, 3);
  const unblocked = pick(3, 2);
  const booked = pick(0, 4);
  await api("POST", `/v1/events/${event.key}/release`, key, { objects: released });
  await give(key, event.key, unblocked, 0);
  const last = await give(key, event.key, booked, 2);
  for (const l of released) now[labels.indexOf(l)] = 0;
  for (const l of unblocked) now[labels.indexOf(l)] = 0;
  for (const l of booked) now[labels.indexOf(l)] = 2;
  const second = await stateAt(key, event.key, snap.seq, last);
  if (second.frame.type !== "delta") throw new Error("expected a delta");
  const delta = second.frame;
  const applied = Uint8Array.from(snap.codes);
  delta.idx.forEach((idx, k) => (applied[idx] = delta.codes[k] ?? 0));
  applied.forEach((c, i) => {
    if (c !== now[i]) throw new Error(`after the delta ${labels[i]} is ${c}, not ${now[i]}`);
  });

  const byLabel = (codes: number[]) =>
    Object.fromEntries(labels.flatMap((l, i) => (codes[i] === 0 ? [] : [[l, codes[i]]])));
  writeFileSync(`${out}${name}.ccf`, chart);
  writeFileSync(`${out}${name}.snapshot`, first.bytes);
  writeFileSync(`${out}${name}.delta`, second.bytes);
  writeFileSync(
    `${out}${name}.json`,
    `${JSON.stringify({
      objects: labels.length,
      snapshot: { seq: snap.seq, encoding: encodingOf(first.bytes), statuses: byLabel(want) },
      delta: {
        from: delta.from,
        to: delta.to,
        changes: Object.fromEntries(
          labels.flatMap((l, i) => (now[i] === want[i] ? [] : [[l, now[i]]])),
        ),
      },
    })}\n`,
  );
  console.log(
    `${name}: ${labels.length} objects, snapshot ${first.bytes.length} B, delta ${second.bytes.length} B`,
  );
}

/** A snapshot's encoding byte: after the type, `seq` and the object count. */
function encodingOf(bytes: Uint8Array): number {
  let at = 1;
  for (let field = 0; field < 2; field++) while ((bytes[at++] ?? 0) >= 0x80);
  return bytes[at] ?? -1;
}

const theatre = JSON.parse(shell([cli, "gen", "theatre"])) as unknown;
/** Without its picture nodes: their asset is not in a new workspace. */
const noImages = (nodes: unknown[]): unknown[] =>
  nodes
    .filter((n) => (n as { type?: string }).type !== "image")
    .map((n) => {
      const children = (n as { children?: unknown[] }).children;
      return children === undefined ? n : { ...(n as object), children: noImages(children) };
    });
const sample = JSON.parse(
  await Bun.file(
    new URL("../../../../../../crates/cavea-compiler/samples/schema-1/full.json", import.meta.url),
  ).text(),
) as { nodes: unknown[] };
const full = { ...sample, nodes: noImages(sample.nodes) };
// Scattered statuses: one code byte per object (encoding 0).
await make("theatre", theatre, (i) => [0, 1, 2, 3, 4 + (i % 12), 0, 2][(i * 7919) % 7] ?? 0);
// Long runs: (code, run) pairs (encoding 1).
await make("full", full, (i) => [0, 2, 3, 0, 5, 1][Math.floor(i / 9) % 6] ?? 0);
