// The staff state route's frames (08 §8.1, §8.6; D-72), decoded in the
// manager view: codes are domain statuses 0 to 15, snapshot encodings 0 and
// 1 only (a manager decoder refuses encoding 2). Every byte is hostile: a
// frame that breaks any rule of 08 §8.6 is refused whole with a `FrameError`
// and nothing of it is used; nothing here throws anything else, and no count
// in the bytes drives an allocation larger than the frame could describe.

export const SNAPSHOT = 0x02;
export const DELTA = 0x03;
/** The largest object count and list count (02 §14). */
export const MAX_OBJECTS = 1_000_000;
/** The highest domain status (02 §4.1). */
export const MAX_STATUS = 15;

export class FrameError extends Error {
  constructor(readonly rule: string) {
    super(`malformed manager frame: ${rule}`);
    this.name = "FrameError";
  }
}

export interface SnapshotFrame {
  type: "snapshot";
  seq: number;
  /** One domain status per object, in `idx` order. */
  codes: Uint8Array;
}

export interface DeltaFrame {
  type: "delta";
  from: number;
  to: number;
  /** Changed objects, `idx` strictly increasing. */
  idx: Uint32Array;
  codes: Uint8Array;
  /** One past the highest `idx` named, changes and areas alike (0 when
   * none): a receiver refuses the frame when it passes its count. */
  end: number;
}

/** A frame of a type the route does not send: skipped (08 §8.6). */
export interface OtherFrame {
  type: "other";
  byte: number;
}

export type Frame = SnapshotFrame | DeltaFrame | OtherFrame;

class Reader {
  at = 0;
  constructor(readonly bytes: Uint8Array) {}

  get left(): number {
    return this.bytes.length - this.at;
  }

  u8(): number {
    const b = this.bytes[this.at];
    if (b === undefined) throw new FrameError("truncated");
    this.at += 1;
    return b;
  }

  /** LEB128 of at most 10 bytes; refused above 2^53 (a `seq` the page
   * could not hold exactly), which no server reaches. */
  varint(): number {
    let value = 0;
    for (let i = 0, scale = 1; i < 10; i++, scale *= 128) {
      const b = this.u8();
      if (i === 9 && b > 1) throw new FrameError("varint");
      value += (b & 0x7f) * scale;
      if (b < 0x80) {
        if (!Number.isSafeInteger(value)) throw new FrameError("varint");
        return value;
      }
    }
    throw new FrameError("varint");
  }

  u32(): number {
    const v = this.varint();
    if (v > 0xffff_ffff) throw new FrameError("varint");
    return v;
  }

  /** An object or list count, at most `MAX_OBJECTS`. */
  count(): number {
    const n = this.u32();
    if (n > MAX_OBJECTS) throw new FrameError("count");
    return n;
  }
}

/** A list (08 §8 `list(F)`): `idx` strictly increasing, each below
 * `MAX_OBJECTS`; `field` reads what follows each `idx`. */
function list(r: Reader, minItem: number, field: (r: Reader) => number) {
  const n = r.count();
  if (n * minItem > r.left) throw new FrameError("count");
  const idx = new Uint32Array(n);
  const values = new Uint32Array(n);
  let prev = -1;
  for (let i = 0; i < n; i++) {
    const diff = r.u32();
    if (i > 0 && diff === 0) throw new FrameError("not increasing");
    const at = i === 0 ? diff : prev + diff;
    if (at >= MAX_OBJECTS) throw new FrameError("idx");
    idx[i] = at;
    values[i] = field(r);
    prev = at;
  }
  return { idx, values, end: n === 0 ? 0 : prev + 1 };
}

const code = (r: Reader): number => {
  const c = r.u8();
  if (c > MAX_STATUS) throw new FrameError("code");
  return c;
};

function snapshot(r: Reader): SnapshotFrame {
  const seq = r.varint();
  const objects = r.count();
  const encoding = r.u8();
  const len = r.u32();
  if (len > r.left) throw new FrameError("length");
  const codes = new Uint8Array(objects);
  const state = new Reader(r.bytes.subarray(r.at, r.at + len));
  r.at += len;
  if (encoding === 0) {
    if (len !== objects) throw new FrameError("count");
    for (let i = 0; i < objects; i++) codes[i] = code(state);
  } else if (encoding === 1) {
    let at = 0;
    while (state.left > 0) {
      const c = code(state);
      const run = state.u32();
      if (run === 0 || run > objects - at) throw new FrameError("run");
      codes.fill(c, at, at + run);
      at += run;
    }
    if (at !== objects) throw new FrameError("run");
  } else {
    // 2 is the public one-bit encoding: never on a manager frame.
    throw new FrameError("encoding");
  }
  const areas = list(r, 2, (x) => x.u32());
  if (areas.end > objects) throw new FrameError("idx");
  return { type: "snapshot", seq, codes };
}

function delta(r: Reader): DeltaFrame {
  const from = r.varint();
  const to = r.varint();
  if (to <= from) throw new FrameError("seq");
  const changes = list(r, 2, code);
  const areas = list(r, 2, (x) => x.u32());
  return {
    type: "delta",
    from,
    to,
    idx: changes.idx,
    codes: Uint8Array.from(changes.values),
    end: Math.max(changes.end, areas.end),
  };
}

/** Decodes one frame; throws `FrameError` when it breaks a rule. */
export function decodeFrame(bytes: Uint8Array): Frame {
  const r = new Reader(bytes);
  const type = r.u8();
  let frame: Frame;
  if (type === SNAPSHOT) frame = snapshot(r);
  else if (type === DELTA) frame = delta(r);
  else return { type: "other", byte: type };
  if (r.left !== 0) throw new FrameError("trailing bytes");
  return frame;
}
