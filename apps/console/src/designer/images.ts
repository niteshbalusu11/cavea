// The image tool (07 §4, DSN-020): a picked file is checked as the server
// checks it, named by its content hash (the server's own: BLAKE3, in the
// designer's module), placed at once as an image node, and uploaded in the
// background. The node never waits for the network, and the server never
// holds a draft naming an image it does not hold: a save waits until the
// uploads it depends on are done (`Uploads.settled`). No DOM here: the clock
// and the request are given, so the machine is tested without a browser.
import type { SaveReply } from "./autosave.ts";

/** The server's limit on an upload's bytes by default (08 §13
 * `CAVEA_ASSET_MAX_BYTES`): a larger file is refused before it is read. */
export const MAX_BYTES = 10 * 1024 * 1024;
/** The server's limit on a side, in pixels (08 §6.2, D-49). */
export const MAX_SIDE = 8_192;

export type ImageType = "image/png" | "image/jpeg" | "image/webp";

/** The type the bytes are, by their signature (as the server decides it:
 * the name and the declared type are ignored); `null` for any other. */
export function sniff(head: Uint8Array): ImageType | null {
  const at = (i: number) => head[i] ?? -1;
  const is = (bytes: number[], from = 0) => bytes.every((b, i) => at(from + i) === b);
  if (is([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  if (is([0xff, 0xd8, 0xff])) return "image/jpeg";
  // "RIFF" … "WEBP"
  if (is([0x52, 0x49, 0x46, 0x46]) && is([0x57, 0x45, 0x42, 0x50], 8)) return "image/webp";
  return null;
}

/**
 * The size an image's header declares, in pixels, or `null` when the header
 * does not parse. Used only to refuse a file before the browser decodes it
 * (a small file can declare a huge picture); the size used is the decoded
 * one.
 */
export function declaredSize(
  bytes: Uint8Array,
  type: ImageType,
): { width: number; height: number } | null {
  const at = (i: number) => bytes[i] ?? 0;
  const be16 = (i: number) => (at(i) << 8) | at(i + 1);
  const le16 = (i: number) => at(i) | (at(i + 1) << 8);
  const le24 = (i: number) => at(i) | (at(i + 1) << 8) | (at(i + 2) << 16);
  const be32 = (i: number) => be16(i) * 65_536 + be16(i + 2);
  if (bytes.length < 30) return null;
  switch (type) {
    case "image/png":
      // IHDR is the first chunk: its width and height follow its type.
      return String.fromCharCode(at(12), at(13), at(14), at(15)) === "IHDR"
        ? { width: be32(16), height: be32(20) }
        : null;
    case "image/jpeg": {
      // Segments up to the first frame header (SOF0 to SOF15 but DHT,
      // JPG and DAC): its height and width.
      let i = 2;
      while (i + 9 < bytes.length && at(i) === 0xff) {
        const marker = at(i + 1);
        if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
          return { width: be16(i + 7), height: be16(i + 5) };
        }
        i += 2 + be16(i + 2);
      }
      return null;
    }
    case "image/webp": {
      const chunk = String.fromCharCode(at(12), at(13), at(14), at(15));
      if (chunk === "VP8 ") return { width: le16(26) & 0x3fff, height: le16(28) & 0x3fff };
      if (chunk === "VP8L") {
        const bits = at(21) | (at(22) << 8) | (at(23) << 16) | (at(24) << 24);
        return { width: (bits & 0x3fff) + 1, height: ((bits >>> 14) & 0x3fff) + 1 };
      }
      if (chunk === "VP8X") return { width: le24(24) + 1, height: le24(27) + 1 };
      return null;
    }
  }
}

/** Why a file is not placed. */
export type Refusal =
  | { kind: "tooLarge"; limit: number }
  | { kind: "type" }
  | { kind: "unreadable" }
  | { kind: "dimensions"; limit: number };

/** A file ready to place: its bytes, content hash and size in pixels. */
export interface Checked {
  blob: Blob;
  hash: string;
  width: number;
  height: number;
}

export interface CheckDeps {
  /** The image's size in pixels, decoded by the browser; rejects for bytes
   * it cannot decode. */
  measure(blob: Blob): Promise<{ width: number; height: number }>;
  /** The content hash (`Designer.assetHash`). */
  hash(bytes: Uint8Array): string;
}

/**
 * Checks a picked file as the server will (08 §6.2): at most `MAX_BYTES`
 * (the size the browser read, not anything the file says), PNG, JPEG or
 * WebP by its bytes, each side 1 to `MAX_SIDE` pixels as declared (before
 * decoding, so a small file declaring a huge picture is never decoded) and
 * as decoded. Reads the bytes only once their count is within the limit.
 */
export async function checkImage(
  file: Blob,
  deps: CheckDeps,
): Promise<{ ok: Checked } | { refused: Refusal }> {
  if (file.size > MAX_BYTES) return { refused: { kind: "tooLarge", limit: MAX_BYTES } };
  let bytes: Uint8Array<ArrayBuffer>;
  try {
    bytes = new Uint8Array(await file.arrayBuffer());
  } catch {
    return { refused: { kind: "unreadable" } };
  }
  const type = sniff(bytes.subarray(0, 16));
  if (type === null) return { refused: { kind: "type" } };
  const declared = declaredSize(bytes, type);
  if (declared === null) return { refused: { kind: "unreadable" } };
  const fits = (s: { width: number; height: number }) =>
    [s.width, s.height].every((v) => Number.isInteger(v) && v >= 1 && v <= MAX_SIDE);
  if (!fits(declared)) return { refused: { kind: "dimensions", limit: MAX_SIDE } };
  // Typed by its bytes, so the upload's body says what it is.
  const blob = new Blob([bytes], { type });
  let size: { width: number; height: number };
  try {
    size = await deps.measure(blob);
  } catch {
    return { refused: { kind: "unreadable" } };
  }
  if (!fits(size)) return { refused: { kind: "dimensions", limit: MAX_SIDE } };
  return { ok: { blob, hash: deps.hash(bytes), ...size } };
}

/** The answer to one upload. */
export type UploadReply =
  | { kind: "uploaded"; hash: string }
  /** No answer, or the server cannot take it now: tried again later. */
  | { kind: "offline"; retryAfterMs: number | null }
  | { kind: "signedOut" }
  /** Refused for a reason retrying will not change: `message` says it. */
  | { kind: "refused"; message: string };

export interface Clock {
  now(): number;
  set(run: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export interface UploadDeps {
  clock: Clock;
  /** Sends the bytes (`POST /v1/assets`). Never rejects. */
  upload(blob: Blob): Promise<UploadReply>;
  /** Told whenever the summary changes. */
  onChange(summary: UploadSummary): void;
  /** An upload the server refused: the nodes naming it should go. */
  onRefused(hash: string, message: string): void;
}

export interface UploadSummary {
  /** Uploads on their way. */
  sending: number;
  /** Uploads waiting to be tried again, and when the next is. */
  waiting: number;
  retryAt: number | null;
  signedOut: boolean;
}

type Upload =
  | { kind: "sending" }
  | { kind: "waiting"; at: number; attempts: number; signedOut: boolean }
  | { kind: "done" }
  | { kind: "refused" };

/** Retries wait 2 s, 4 s, … up to this, as saves do. */
export const MAX_RETRY_MS = 60_000;

/**
 * The uploads of a server chart's images, one per content hash. Each is
 * sent once; one that gets no answer waits and is tried again (2 s, 4 s, …
 * up to a minute, or `Retry-After`, or at once on `retryNow`); one the
 * server refuses is reported and never sent again; one done is never sent
 * again (an asset is immutable and kept).
 */
export class Uploads {
  readonly #deps: UploadDeps;
  readonly #blobs = new Map<string, Blob>();
  readonly #state = new Map<string, Upload>();
  #timer: unknown = null;
  #waiters: (() => void)[] = [];
  #disposed = false;

  constructor(deps: UploadDeps) {
    this.#deps = deps;
  }

  /** Uploads `blob`, whose content hash is `hash`, unless it is on its
   * way, done or refused. A hash waiting is tried again now. */
  add(hash: string, blob: Blob): void {
    const s = this.#state.get(hash);
    if (s?.kind === "sending" || s?.kind === "done" || s?.kind === "refused") return;
    this.#blobs.set(hash, blob);
    this.#send(hash, s?.kind === "waiting" ? s.attempts : 0);
  }

  /** Tries every waiting upload now (Retry; the browser back online). */
  retryNow(): void {
    for (const [hash, s] of this.#state) {
      if (s.kind === "waiting") this.#send(hash, s.attempts);
    }
  }

  state(hash: string): Upload["kind"] | null {
    return this.#state.get(hash)?.kind ?? null;
  }

  summary(): UploadSummary {
    let sending = 0;
    let waiting = 0;
    let retryAt: number | null = null;
    let signedOut = false;
    for (const s of this.#state.values()) {
      if (s.kind === "sending") sending++;
      if (s.kind === "waiting") {
        waiting++;
        retryAt = retryAt === null ? s.at : Math.min(retryAt, s.at);
        signedOut ||= s.signedOut;
      }
    }
    return { sending, waiting, retryAt, signedOut };
  }

  /**
   * Waits until no upload is on its way, then says whether a draft may be
   * saved: `null` when every upload is done or refused; otherwise the
   * reply a save gets for now (`offline`, retried when the first waiting
   * upload is; `signedOut`), so the save machine shows it and keeps the
   * draft in the browser.
   */
  async settled(): Promise<SaveReply | null> {
    while (this.summary().sending > 0 && !this.#disposed) {
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
    const s = this.summary();
    if (s.signedOut) return { kind: "signedOut" };
    if (s.waiting === 0) return null;
    const now = this.#deps.clock.now();
    return { kind: "offline", retryAfterMs: Math.max(0, (s.retryAt ?? now) - now) };
  }

  dispose(): void {
    this.#disposed = true;
    if (this.#timer !== null) this.#deps.clock.clear(this.#timer);
    this.#timer = null;
    this.#wake();
  }

  #send(hash: string, attempts: number): void {
    const blob = this.#blobs.get(hash);
    if (blob === undefined || this.#disposed) return;
    this.#state.set(hash, { kind: "sending" });
    this.#changed();
    void this.#deps.upload(blob).then((reply) => {
      if (this.#disposed) return;
      switch (reply.kind) {
        case "uploaded":
          if (reply.hash === hash) {
            this.#state.set(hash, { kind: "done" });
            this.#blobs.delete(hash);
          } else {
            // The server names the bytes otherwise: a node naming `hash`
            // would never find them.
            this.#refuse(hash, `the server named the image ${reply.hash}`);
          }
          break;
        case "refused":
          this.#refuse(hash, reply.message);
          break;
        case "offline":
        case "signedOut": {
          const wait =
            reply.kind === "offline" && reply.retryAfterMs !== null
              ? reply.retryAfterMs
              : Math.min(2_000 * 2 ** attempts, MAX_RETRY_MS);
          const at = this.#deps.clock.now() + wait;
          const signedOut = reply.kind === "signedOut";
          this.#state.set(hash, { kind: "waiting", at, attempts: attempts + 1, signedOut });
          this.#schedule();
          break;
        }
      }
      this.#changed();
      this.#wake();
    });
  }

  #refuse(hash: string, message: string): void {
    this.#state.set(hash, { kind: "refused" });
    this.#blobs.delete(hash);
    this.#deps.onRefused(hash, message);
  }

  /** One timer, for the earliest waiting upload. */
  #schedule(): void {
    if (this.#timer !== null) this.#deps.clock.clear(this.#timer);
    this.#timer = null;
    const at = this.summary().retryAt;
    if (at === null) return;
    this.#timer = this.#deps.clock.set(
      () => {
        this.#timer = null;
        const now = this.#deps.clock.now();
        for (const [hash, s] of this.#state) {
          if (s.kind === "waiting" && !s.signedOut && s.at <= now) this.#send(hash, s.attempts);
        }
        this.#schedule();
      },
      Math.max(0, at - this.#deps.clock.now()),
    );
  }

  #changed(): void {
    this.#deps.onChange(this.summary());
  }

  #wake(): void {
    for (const w of this.#waiters.splice(0)) w();
  }
}

/** Every image node naming `hash` in a chart document (sections' children
 * included): the ids to delete when its upload was refused. */
export function nodesNaming(document: string, hash: string): number[] {
  const ids: number[] = [];
  const walk = (nodes: unknown): void => {
    if (!Array.isArray(nodes)) return;
    for (const n of nodes as Record<string, unknown>[]) {
      if (n["type"] === "image" && n["asset"] === hash && typeof n["id"] === "number") {
        ids.push(n["id"]);
      }
      walk(n["children"]);
    }
  };
  try {
    walk((JSON.parse(document) as Record<string, unknown>)["nodes"]);
  } catch {
    // The session's own export always parses.
  }
  return ids;
}
