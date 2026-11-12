// A QR code encoder for one short text (D-92): the `otpauth://` URI an
// authenticator app scans when a second factor is turned on (CON-003). Our
// own, rather than a package, because it handles a secret and is small and
// fully specified (ISO/IEC 18004): byte mode, error correction level M,
// the smallest of versions 1 to 40 that holds the text, Reed-Solomon over
// GF(256), and the mask with the lowest penalty. The structure follows
// Project Nayuki's reference implementation (MIT). Tests compare its
// matrices with ones another encoder made (`qr.test.ts`).
//
// It never throws: a text too long for version 40 gives `null`, and the
// page shows the secret as text alone.

export interface QrCode {
  version: number;
  /** Modules per side: 17 + 4 × version. */
  size: number;
  /** The mask chosen, 0 to 7. */
  mask: number;
  /** Row-major, `true` for a dark module; `size × size` entries. */
  modules: boolean[];
}

// Level M's error correction codewords per block and number of blocks, by
// version (index 0 unused).
const ECC_PER_BLOCK = [
  -1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28,
  28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
];
const BLOCKS = [
  -1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25,
  26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
];
/** Level M's two format bits. */
const LEVEL_M = 0;

const at = (list: readonly number[], i: number): number => list[i] ?? 0;

/** Modules that carry data or error correction in a version. */
function rawModules(version: number): number {
  let n = (16 * version + 128) * version + 64;
  if (version >= 2) {
    const align = Math.floor(version / 7) + 2;
    n -= (25 * align - 10) * align - 55;
    if (version >= 7) n -= 36;
  }
  return n;
}

const dataCodewords = (version: number): number =>
  Math.floor(rawModules(version) / 8) - at(ECC_PER_BLOCK, version) * at(BLOCKS, version);

// GF(256) with the polynomial x^8 + x^4 + x^3 + x^2 + 1.
function multiply(x: number, y: number): number {
  let z = 0;
  for (let i = 7; i >= 0; i--) {
    z = (z << 1) ^ ((z >>> 7) * 0x11d);
    z ^= ((y >>> i) & 1) * x;
  }
  return z & 0xff;
}

function divisor(degree: number): number[] {
  const result: number[] = new Array<number>(degree).fill(0);
  result[degree - 1] = 1;
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < degree; j++) {
      result[j] = multiply(at(result, j), root);
      if (j + 1 < degree) result[j] = at(result, j) ^ at(result, j + 1);
    }
    root = multiply(root, 0x02);
  }
  return result;
}

function remainder(data: readonly number[], div: readonly number[]): number[] {
  const result: number[] = div.map(() => 0);
  for (const b of data) {
    const factor = b ^ (result.shift() ?? 0);
    result.push(0);
    div.forEach((coef, i) => {
      result[i] = at(result, i) ^ multiply(coef, factor);
    });
  }
  return result;
}

/** The data codewords split into blocks, each given its error correction,
 * and interleaved. */
function withErrorCorrection(data: readonly number[], version: number): number[] {
  const blocks = at(BLOCKS, version);
  const eccLength = at(ECC_PER_BLOCK, version);
  const raw = Math.floor(rawModules(version) / 8);
  const shortBlocks = blocks - (raw % blocks);
  const shortLength = Math.floor(raw / blocks);
  const div = divisor(eccLength);
  const all: number[][] = [];
  for (let i = 0, k = 0; i < blocks; i++) {
    const block = data.slice(k, k + shortLength - eccLength + (i < shortBlocks ? 0 : 1));
    k += block.length;
    const ecc = remainder(block, div);
    if (i < shortBlocks) block.push(0);
    all.push(block.concat(ecc));
  }
  const result: number[] = [];
  const width = all[0]?.length ?? 0;
  for (let i = 0; i < width; i++) {
    all.forEach((block, j) => {
      // The padding byte of a short block is not sent.
      if (i !== shortLength - eccLength || j >= shortBlocks) result.push(at(block, i));
    });
  }
  return result;
}

/** The text's codewords at `version`: byte mode, its length, the bytes, a
 * terminator and padding. */
function codewords(bytes: Uint8Array, version: number): number[] {
  const bits: number[] = [];
  const put = (value: number, length: number) => {
    for (let i = length - 1; i >= 0; i--) bits.push((value >>> i) & 1);
  };
  put(0b0100, 4);
  put(bytes.length, version < 10 ? 8 : 16);
  for (const b of bytes) put(b, 8);
  const capacity = dataCodewords(version) * 8;
  put(0, Math.min(4, capacity - bits.length));
  put(0, (8 - (bits.length % 8)) % 8);
  for (let pad = 0xec; bits.length < capacity; pad ^= 0xec ^ 0x11) put(pad, 8);
  const out: number[] = [];
  for (let i = 0; i < bits.length; i += 8) {
    let byte = 0;
    for (let j = 0; j < 8; j++) byte = (byte << 1) | at(bits, i + j);
    out.push(byte);
  }
  return out;
}

function alignmentPositions(version: number, size: number): number[] {
  if (version === 1) return [];
  const count = Math.floor(version / 7) + 2;
  const step = Math.floor((version * 8 + count * 3 + 5) / (count * 4 - 4)) * 2;
  const result = [6];
  for (let pos = size - 7; result.length < count; pos -= step) result.splice(1, 0, pos);
  return result;
}

class Grid {
  readonly modules: boolean[];
  readonly fixed: boolean[];
  constructor(readonly size: number) {
    this.modules = new Array<boolean>(size * size).fill(false);
    this.fixed = new Array<boolean>(size * size).fill(false);
  }
  get(x: number, y: number): boolean {
    return this.modules[y * this.size + x] === true;
  }
  /** A module of a function pattern: never masked, never data. */
  setFixed(x: number, y: number, dark: boolean): void {
    if (x < 0 || y < 0 || x >= this.size || y >= this.size) return;
    this.modules[y * this.size + x] = dark;
    this.fixed[y * this.size + x] = true;
  }
}

const bit = (value: number, i: number): boolean => ((value >>> i) & 1) !== 0;

function drawFormat(grid: Grid, mask: number): void {
  const data = (LEVEL_M << 3) | mask;
  let rem = data;
  for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
  const bits = ((data << 10) | rem) ^ 0x5412;
  const size = grid.size;
  for (let i = 0; i <= 5; i++) grid.setFixed(8, i, bit(bits, i));
  grid.setFixed(8, 7, bit(bits, 6));
  grid.setFixed(8, 8, bit(bits, 7));
  grid.setFixed(7, 8, bit(bits, 8));
  for (let i = 9; i < 15; i++) grid.setFixed(14 - i, 8, bit(bits, i));
  for (let i = 0; i < 8; i++) grid.setFixed(size - 1 - i, 8, bit(bits, i));
  for (let i = 8; i < 15; i++) grid.setFixed(8, size - 15 + i, bit(bits, i));
  grid.setFixed(8, size - 8, true);
}

function drawFunctionPatterns(grid: Grid, version: number): void {
  const size = grid.size;
  for (let i = 0; i < size; i++) {
    grid.setFixed(6, i, i % 2 === 0);
    grid.setFixed(i, 6, i % 2 === 0);
  }
  for (const [cx, cy] of [
    [3, 3],
    [size - 4, 3],
    [3, size - 4],
  ] as const) {
    for (let dy = -4; dy <= 4; dy++) {
      for (let dx = -4; dx <= 4; dx++) {
        const d = Math.max(Math.abs(dx), Math.abs(dy));
        grid.setFixed(cx + dx, cy + dy, d !== 2 && d !== 4);
      }
    }
  }
  const align = alignmentPositions(version, size);
  const last = align.length - 1;
  align.forEach((cx, i) => {
    align.forEach((cy, j) => {
      if ((i === 0 && j === 0) || (i === 0 && j === last) || (i === last && j === 0)) return;
      for (let dy = -2; dy <= 2; dy++) {
        for (let dx = -2; dx <= 2; dx++) {
          grid.setFixed(cx + dx, cy + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
        }
      }
    });
  });
  drawFormat(grid, 0);
  if (version >= 7) {
    let rem = version;
    for (let i = 0; i < 12; i++) rem = (rem << 1) ^ ((rem >>> 11) * 0x1f25);
    const bits = (version << 12) | rem;
    for (let i = 0; i < 18; i++) {
      const a = size - 11 + (i % 3);
      const b = Math.floor(i / 3);
      grid.setFixed(a, b, bit(bits, i));
      grid.setFixed(b, a, bit(bits, i));
    }
  }
}

function drawData(grid: Grid, data: readonly number[]): void {
  const size = grid.size;
  let i = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let vert = 0; vert < size; vert++) {
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const upward = ((right + 1) & 2) === 0;
        const y = upward ? size - 1 - vert : vert;
        const k = y * size + x;
        if (!grid.fixed[k] && i < data.length * 8) {
          grid.modules[k] = bit(at(data, i >>> 3), 7 - (i & 7));
          i++;
        }
      }
    }
  }
}

function masked(mask: number, x: number, y: number): boolean {
  switch (mask) {
    case 0:
      return (x + y) % 2 === 0;
    case 1:
      return y % 2 === 0;
    case 2:
      return x % 3 === 0;
    case 3:
      return (x + y) % 3 === 0;
    case 4:
      return (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0;
    case 5:
      return ((x * y) % 2) + ((x * y) % 3) === 0;
    case 6:
      return (((x * y) % 2) + ((x * y) % 3)) % 2 === 0;
    default:
      return (((x + y) % 2) + ((x * y) % 3)) % 2 === 0;
  }
}

/** Applies (or, applied again, removes) a mask over the data modules. */
function applyMask(grid: Grid, mask: number): void {
  const size = grid.size;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const k = y * size + x;
      if (!grid.fixed[k] && masked(mask, x, y)) grid.modules[k] = !grid.modules[k];
    }
  }
}

/** The standard's penalty for a masked symbol (lower is better). */
function penalty(grid: Grid): number {
  const size = grid.size;
  let result = 0;
  const addHistory = (run: number, history: number[]) => {
    if (history[0] === 0) run += size;
    history.pop();
    history.unshift(run);
  };
  const countFinders = (h: number[]): number => {
    const n = at(h, 1);
    const core = n > 0 && h[2] === n && h[3] === n * 3 && h[4] === n && h[5] === n;
    return (
      (core && at(h, 0) >= n * 4 && at(h, 6) >= n ? 1 : 0) +
      (core && at(h, 6) >= n * 4 && at(h, 0) >= n ? 1 : 0)
    );
  };
  const line = (get: (i: number) => boolean) => {
    let color = false;
    let run = 0;
    const history = [0, 0, 0, 0, 0, 0, 0];
    for (let i = 0; i < size; i++) {
      if (get(i) === color) {
        run++;
        if (run === 5) result += 3;
        else if (run > 5) result++;
      } else {
        addHistory(run, history);
        if (!color) result += countFinders(history) * 40;
        color = get(i);
        run = 1;
      }
    }
    if (color) {
      addHistory(run, history);
      run = 0;
    }
    run += size;
    addHistory(run, history);
    result += countFinders(history) * 40;
  };
  for (let y = 0; y < size; y++) line((x) => grid.get(x, y));
  for (let x = 0; x < size; x++) line((y) => grid.get(x, y));
  for (let y = 0; y < size - 1; y++) {
    for (let x = 0; x < size - 1; x++) {
      const c = grid.get(x, y);
      if (c === grid.get(x + 1, y) && c === grid.get(x, y + 1) && c === grid.get(x + 1, y + 1)) {
        result += 3;
      }
    }
  }
  const dark = grid.modules.filter(Boolean).length;
  const total = size * size;
  result += (Math.ceil(Math.abs(dark * 20 - total * 10) / total) - 1) * 10;
  return result;
}

/** The text as a QR code at level M, or `null` when it is too long for
 * version 40. `mask` forces a mask (tests compare with a reference). */
export function encodeQr(text: string, mask?: number): QrCode | null {
  const bytes = new TextEncoder().encode(text);
  let version = 1;
  for (; version <= 40; version++) {
    const header = 4 + (version < 10 ? 8 : 16);
    if (header + bytes.length * 8 <= dataCodewords(version) * 8) break;
  }
  if (version > 40) return null;
  const size = version * 4 + 17;
  const grid = new Grid(size);
  drawFunctionPatterns(grid, version);
  drawData(grid, withErrorCorrection(codewords(bytes, version), version));
  let chosen = mask !== undefined && mask >= 0 && mask <= 7 ? Math.floor(mask) : -1;
  if (chosen < 0) {
    let best = Infinity;
    for (let m = 0; m < 8; m++) {
      applyMask(grid, m);
      drawFormat(grid, m);
      const score = penalty(grid);
      if (score < best) {
        best = score;
        chosen = m;
      }
      applyMask(grid, m);
    }
  }
  applyMask(grid, chosen);
  drawFormat(grid, chosen);
  return { version, size, mask: chosen, modules: grid.modules };
}

/** An SVG path of the dark modules, one unit each, offset by `margin`. */
export function qrPath(code: QrCode, margin: number): string {
  const parts: string[] = [];
  for (let y = 0; y < code.size; y++) {
    for (let x = 0; x < code.size; x++) {
      if (code.modules[y * code.size + x] === true) {
        parts.push(`M${x + margin} ${y + margin}h1v1h-1z`);
      }
    }
  }
  return parts.join("");
}
