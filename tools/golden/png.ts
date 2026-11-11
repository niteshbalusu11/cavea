// PNG for the golden images: 8-bit RGB written, 8-bit RGB or RGBA read
// (what this harness writes, and what an image tool re-saving it does), with
// bun's zlib. Pixels are RGBA in memory. Anything else is refused.
import { deflateSync, inflateSync } from "node:zlib";

export interface Image {
  width: number;
  height: number;
  /** RGBA, rows top to bottom. */
  rgba: Uint8Array;
}

const SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

function chunk(type: string, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(12 + data.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, data.length);
  out.set(new TextEncoder().encode(type), 4);
  out.set(data, 8);
  view.setUint32(8 + data.length, Bun.hash.crc32(out.subarray(4, 8 + data.length)));
  return out;
}

const paeth = (a: number, b: number, c: number): number => {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
};

/** The filtered byte at `i` of a row with filter `type`: `x` the raw byte, `a` left, `b` up, `c` up-left. */
function filter(type: number, x: number, a: number, b: number, c: number): number {
  switch (type) {
    case 1:
      return (x - a) & 255;
    case 2:
      return (x - b) & 255;
    case 3:
      return (x - ((a + b) >> 1)) & 255;
    case 4:
      return (x - paeth(a, b, c)) & 255;
    default:
      return x;
  }
}

/** Encodes opaque RGBA as 8-bit RGB, each row with the filter of smallest absolute sum. */
export function encode({ width, height, rgba }: Image): Uint8Array {
  const stride = width * 3;
  const raw = new Uint8Array((stride + 1) * height);
  let prev = new Uint8Array(stride);
  const row = new Uint8Array(stride);
  const best = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      for (let k = 0; k < 3; k++) {
        row[x * 3 + k] = rgba[(y * width + x) * 4 + k] ?? 0;
      }
    }
    let bestType = 0;
    let bestSum = Infinity;
    const out = new Uint8Array(stride);
    for (let type = 0; type < 5; type++) {
      let sum = 0;
      for (let i = 0; i < stride; i++) {
        const a = i >= 3 ? (row[i - 3] ?? 0) : 0;
        const c = i >= 3 ? (prev[i - 3] ?? 0) : 0;
        const v = filter(type, row[i] ?? 0, a, prev[i] ?? 0, c);
        out[i] = v;
        sum += v < 128 ? v : 256 - v;
      }
      if (sum < bestSum) {
        bestSum = sum;
        bestType = type;
        best.set(out);
      }
    }
    raw[y * (stride + 1)] = bestType;
    raw.set(best, y * (stride + 1) + 1);
    prev = row.slice();
  }
  const header = new Uint8Array(13);
  const view = new DataView(header.buffer);
  view.setUint32(0, width);
  view.setUint32(4, height);
  header.set([8, 2, 0, 0, 0], 8);
  const parts = [
    Uint8Array.from(SIGNATURE),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", new Uint8Array(0)),
  ];
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

/** Decodes an 8-bit RGB or RGBA, non-interlaced PNG; throws on anything else. */
export function decode(bytes: Uint8Array): Image {
  if (!SIGNATURE.every((b, i) => bytes[i] === b)) {
    throw new Error("not a PNG");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let at = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const idat: Uint8Array[] = [];
  while (at + 8 <= bytes.length) {
    const length = view.getUint32(at);
    const type = new TextDecoder().decode(bytes.subarray(at + 4, at + 8));
    const data = bytes.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = view.getUint32(at + 8);
      height = view.getUint32(at + 12);
      const [depth, colour, , , interlace] = data.subarray(8);
      channels = colour === 2 ? 3 : colour === 6 ? 4 : 0;
      if (depth !== 8 || channels === 0 || interlace !== 0) {
        throw new Error(
          `unsupported PNG: depth ${depth}, colour type ${colour}, interlace ${interlace}`,
        );
      }
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  if (width === 0 || raw.length !== (stride + 1) * height) {
    throw new Error("PNG data does not match its header");
  }
  const rgba = new Uint8Array(width * height * 4);
  let prev = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const type = raw[y * (stride + 1)] ?? 0;
    const row = new Uint8Array(stride);
    for (let i = 0; i < stride; i++) {
      const x = raw[y * (stride + 1) + 1 + i] ?? 0;
      const a = i >= channels ? (row[i - channels] ?? 0) : 0;
      const b = prev[i] ?? 0;
      const c = i >= channels ? (prev[i - channels] ?? 0) : 0;
      // Unfiltering adds where filtering subtracts.
      const predictor = type === 0 ? 0 : (x - filter(type, x, a, b, c)) & 255;
      row[i] = (x + predictor) & 255;
    }
    for (let x = 0; x < width; x++) {
      for (let k = 0; k < 4; k++) {
        rgba[(y * width + x) * 4 + k] = k < channels ? (row[x * channels + k] ?? 0) : 255;
      }
    }
    prev = row;
  }
  return { width, height, rgba };
}
