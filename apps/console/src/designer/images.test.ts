// The image tool's checks and its upload machine, with a fake request
// layer and clock: what is refused before an upload, and that a save never
// goes ahead of an upload it depends on.
import { describe, expect, test } from "bun:test";
import type { SaveReply } from "./autosave.ts";
import {
  checkImage,
  declaredSize,
  MAX_BYTES,
  nodesNaming,
  sniff,
  Uploads,
  type UploadReply,
  type UploadSummary,
} from "./images.ts";

/** A PNG header declaring `w × h` (the rest of the file does not matter
 * to the checks: the fake decoder says what the browser would). */
function png(w: number, h: number): Uint8Array {
  const b = new Uint8Array(64);
  b.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
  new DataView(b.buffer).setUint32(16, w);
  new DataView(b.buffer).setUint32(20, h);
  return b;
}

describe("checks before placing", () => {
  const deps = (size: { width: number; height: number } | null) => ({
    measure: () => (size === null ? Promise.reject(new Error("bad")) : Promise.resolve(size)),
    hash: () => "0".repeat(32),
  });

  test("the type is the bytes', whatever the name or declared type", () => {
    expect(sniff(png(1, 1))).toBe("image/png");
    expect(sniff(new Uint8Array([0xff, 0xd8, 0xff, 0xe0]))).toBe("image/jpeg");
    const webp = new TextEncoder().encode("RIFF\0\0\0\0WEBPVP8 ");
    expect(sniff(webp)).toBe("image/webp");
    expect(sniff(new TextEncoder().encode("<svg xmlns="))).toBeNull();
    expect(sniff(new Uint8Array(0))).toBeNull();
  });

  test("declared sizes of the three formats", () => {
    expect(declaredSize(png(800, 600), "image/png")).toEqual({ width: 800, height: 600 });
    // A JPEG with an APP0 segment, then SOF0: height 300, width 400.
    const jpeg = new Uint8Array(40);
    jpeg.set([0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 1, 44, 1, 144]);
    expect(declaredSize(jpeg, "image/jpeg")).toEqual({ width: 400, height: 300 });
    // VP8X: (w − 1, h − 1) as 24-bit numbers.
    const x = new Uint8Array(40);
    x.set(new TextEncoder().encode("RIFF\0\0\0\0WEBPVP8X"));
    x.set([0xff, 0x1f, 0, 0x0f, 0, 0], 24);
    expect(declaredSize(x, "image/webp")).toEqual({ width: 8192, height: 16 });
    expect(declaredSize(new Uint8Array(40), "image/png")).toBeNull();
  });

  test("refused: too many bytes, another type, a header or picture too large, undecodable", async () => {
    const big = { size: MAX_BYTES + 1, arrayBuffer: () => Promise.reject(new Error("read")) };
    expect(await checkImage(big as unknown as Blob, deps(null))).toEqual({
      refused: { kind: "tooLarge", limit: MAX_BYTES },
    });
    const svg = new Blob(["<svg></svg>".padEnd(64)]);
    expect(await checkImage(svg, deps(null))).toEqual({ refused: { kind: "type" } });
    // Declared too large: never decoded (the fake would say it is fine).
    const bomb = new Blob([png(100_000, 100_000)]);
    expect(await checkImage(bomb, deps({ width: 10, height: 10 }))).toEqual({
      refused: { kind: "dimensions", limit: 8192 },
    });
    // Decoded larger than declared: the decoded size counts.
    const liar = new Blob([png(100, 100)]);
    expect(await checkImage(liar, deps({ width: 9000, height: 100 }))).toEqual({
      refused: { kind: "dimensions", limit: 8192 },
    });
    expect(await checkImage(liar, deps(null))).toEqual({ refused: { kind: "unreadable" } });
    const ok = await checkImage(liar, deps({ width: 100, height: 80 }));
    expect("ok" in ok && ok.ok.blob.type).toBe("image/png");
    expect("ok" in ok && [ok.ok.width, ok.ok.height]).toEqual([100, 80]);
  });

  test("the nodes naming a picture, in sections too", () => {
    const doc = JSON.stringify({
      nodes: [
        { id: 1, type: "image", asset: "a" },
        { id: 2, type: "section", children: [{ id: 3, type: "image", asset: "a" }] },
        { id: 4, type: "image", asset: "b" },
      ],
    });
    expect(nodesNaming(doc, "a")).toEqual([1, 3]);
  });
});

/** A clock run by hand and a request layer whose answers the test gives. */
function rig() {
  let now = 0;
  const timers = new Map<number, { at: number; run: () => void }>();
  let next = 0;
  const sent: { blob: Blob; answer: (r: UploadReply) => void }[] = [];
  const refused: string[] = [];
  const summaries: UploadSummary[] = [];
  const uploads = new Uploads({
    clock: {
      now: () => now,
      set: (run, ms) => {
        timers.set(++next, { at: now + ms, run });
        return next;
      },
      clear: (h) => void timers.delete(h as number),
    },
    upload: (blob) => new Promise((answer) => sent.push({ blob, answer })),
    onChange: (s) => summaries.push(s),
    onRefused: (hash) => refused.push(hash),
  });
  const advance = async (ms: number) => {
    now += ms;
    const due = Array.from(timers).filter(([, t]) => t.at <= now);
    for (const [id, t] of due) {
      timers.delete(id);
      t.run();
    }
    await Promise.resolve();
  };
  const settle = () => new Promise((r) => setTimeout(r, 0));
  return { uploads, sent, refused, summaries, advance, settle };
}

describe("uploads", () => {
  const blob = new Blob(["x"]);
  const H = "a".repeat(32);

  test("a save waits for the upload it depends on, then goes", async () => {
    const r = rig();
    r.uploads.add(H, blob);
    r.uploads.add(H, blob);
    expect(r.sent.length).toBe(1);
    let reply: SaveReply | null | undefined;
    void r.uploads.settled().then((v) => (reply = v));
    await r.settle();
    expect(reply).toBeUndefined();
    r.sent[0]?.answer({ kind: "uploaded", hash: H });
    await r.settle();
    expect(reply).toBeNull();
    expect(r.uploads.state(H)).toBe("done");
    // Done is never sent again.
    r.uploads.add(H, blob);
    expect(r.sent.length).toBe(1);
  });

  test("offline: the save reports offline until a retry gets through", async () => {
    const r = rig();
    r.uploads.add(H, blob);
    r.sent[0]?.answer({ kind: "offline", retryAfterMs: null });
    await r.settle();
    expect(await r.uploads.settled()).toEqual({ kind: "offline", retryAfterMs: 2_000 });
    expect(r.summaries.at(-1)).toMatchObject({ sending: 0, waiting: 1 });
    // Retried after 2 s, then 4 s.
    await r.advance(1_999);
    expect(r.sent.length).toBe(1);
    await r.advance(1);
    expect(r.sent.length).toBe(2);
    r.sent[1]?.answer({ kind: "offline", retryAfterMs: null });
    await r.settle();
    await r.advance(3_999);
    expect(r.sent.length).toBe(2);
    // Back online: at once.
    r.uploads.retryNow();
    expect(r.sent.length).toBe(3);
    r.sent[2]?.answer({ kind: "uploaded", hash: H });
    await r.settle();
    expect(await r.uploads.settled()).toBeNull();
  });

  test("refused: reported once, never sent again, and no longer holds saves", async () => {
    const r = rig();
    r.uploads.add(H, blob);
    r.sent[0]?.answer({ kind: "refused", message: "no" });
    await r.settle();
    expect(r.refused).toEqual([H]);
    r.uploads.add(H, blob);
    r.uploads.retryNow();
    expect(r.sent.length).toBe(1);
    expect(await r.uploads.settled()).toBeNull();
    // A server that names the bytes otherwise is a refusal too.
    const G = "b".repeat(32);
    r.uploads.add(G, blob);
    r.sent[1]?.answer({ kind: "uploaded", hash: H });
    await r.settle();
    expect(r.refused).toEqual([H, G]);
  });

  test("signed out: the save is told, so the work is kept and sign-in follows", async () => {
    const r = rig();
    r.uploads.add(H, blob);
    r.sent[0]?.answer({ kind: "signedOut" });
    await r.settle();
    expect(await r.uploads.settled()).toEqual({ kind: "signedOut" });
    // Not retried by the clock.
    await r.advance(120_000);
    expect(r.sent.length).toBe(1);
  });
});
