// Every object's full label in `idx` order, from a compiled chart (03 §4,
// `cavea_format::full_label`): the staff state route's frames name objects by
// `idx`, the console and the API by label. The SDK keeps its chart to itself,
// so the manager reads the few chunks labels need from the same bytes it
// hands the map. Seat labels are generated as `cavea-core::label` does; the
// test pins them to the server's own label index through frames it encoded.
// The bytes are checked as they are read: anything out of bounds or out of
// its table is a `ChartError`, never an exception of another kind.

export class ChartError extends Error {
  constructor(readonly rule: string) {
    super(`unreadable chart: ${rule}`);
    this.name = "ChartError";
  }
}

const NONE = 0xffff_ffff;
const MAX_OBJECTS = 1_000_000;
/** Labels are at most 64 bytes (02 §14). */
const MAX_LABEL = 64;

/** Chunk tags and their record sizes (03 §4.2.2). */
const SIZES = { META: 41, SECT: 66, ROWS: 60, RSEG: 12, LBLR: 26, OBJS: 53 } as const;

class Cur {
  at: number;
  constructor(
    readonly view: DataView,
    start: number,
    readonly end: number,
  ) {
    this.at = start;
  }
  need(n: number): void {
    if (this.at + n > this.end) throw new ChartError("truncated");
  }
  u8(): number {
    this.need(1);
    return this.view.getUint8(this.at++);
  }
  u16(): number {
    this.need(2);
    const v = this.view.getUint16(this.at, true);
    this.at += 2;
    return v;
  }
  u32(): number {
    this.need(4);
    const v = this.view.getUint32(this.at, true);
    this.at += 4;
    return v;
  }
  /** A varint of at most 5 bytes that fits a u32 (03 §4.1). */
  varint(): number {
    let value = 0;
    for (let i = 0, scale = 1; i < 5; i++, scale *= 128) {
      const b = this.u8();
      value += (b & 0x7f) * scale;
      if (b < 0x80) {
        if (value > 0xffff_ffff) break;
        return value;
      }
    }
    throw new ChartError("varint");
  }
}

interface Row {
  section: number;
  label: number;
  first: number;
  seats: number;
  positions: number;
  rule: number;
  base: number;
  order: number;
}

interface Rule {
  scheme: number;
  overflow: number;
  start: number;
  step: number;
  prefix: number;
  suffix: number;
  customFirst: number;
  customCount: number;
}

const ROMAN: [number, string][] = [
  [1000, "M"],
  [900, "CM"],
  [500, "D"],
  [400, "CD"],
  [100, "C"],
  [90, "XC"],
  [50, "L"],
  [40, "XL"],
  [10, "X"],
  [9, "IX"],
  [5, "V"],
  [4, "IV"],
  [1, "I"],
];

/** `value` in a scheme (`cavea-core::label::write_value`); `null` when it
 * cannot be written. */
function written(scheme: number, overflow: number, value: number): string | null {
  switch (scheme) {
    case 0:
    case 1:
    case 2:
      return String(value);
    case 3:
    case 4: {
      if (value < 1) return null;
      const base = scheme === 3 ? 65 : 97;
      if (overflow === 0) {
        const v = value - 1;
        const repeat = Math.floor(v / 26) + 1;
        if (repeat > MAX_LABEL) return null;
        return String.fromCharCode(base + (v % 26)).repeat(repeat);
      }
      let out = "";
      for (let n = value; n > 0; n = Math.floor(n / 26)) {
        n -= 1;
        out = String.fromCharCode(base + (n % 26)) + out;
      }
      return out;
    }
    case 5: {
      if (value < 1 || value > 3999) return null;
      let out = "";
      let rest = value;
      for (const [v, s] of ROMAN) {
        while (rest >= v) {
          out += s;
          rest -= v;
        }
      }
      return out;
    }
    default:
      return null;
  }
}

/** The ordinal of position `p` in a row of `len` positions
 * (`cavea-core::label::seat_ordinal`, the order already resolved). */
function seatOrdinal(order: number, len: number, p: number): number {
  const last = Math.max(0, len - 1);
  if (order === 1) return Math.max(0, last - p);
  if (order === 2) {
    const centre = Math.floor(last / 2);
    return p <= centre ? (centre - p) * 2 : (p - centre) * 2 - 1;
  }
  return p;
}

/**
 * `template` with `{section}`, `{row}` and `{seat}` filled in one pass, each
 * empty part left out with its separator: the one after it, or the one before
 * it when no written part follows (03 §3.1.1, D-96). The prefix and suffix are
 * always written. The port of `cavea_core::template::write_full_label`,
 * checked against its table (`fixtures/label-cases.json`).
 */
export function compose(template: string, section: string, row: string, seat: string): string {
  const values: Record<string, string> = { section, row, seat };
  const placeholder = /\{(section|row|seat)\}/g;
  let out = "";
  let separator = "";
  let started = false;
  let wrote = false;
  let afterWritten = false;
  let at = 0;
  for (const m of template.matchAll(placeholder)) {
    const literal = template.slice(at, m.index);
    if (!started) {
      out += literal;
      started = true;
    } else if (afterWritten) {
      separator = literal;
    }
    const value = values[m[1] ?? ""] ?? "";
    afterWritten = value !== "";
    if (afterWritten) {
      if (wrote) out += separator;
      out += value;
      wrote = true;
    }
    at = m.index + m[0].length;
  }
  return out + template.slice(at);
}

const utf8 = new TextDecoder("utf-8", { fatal: true });
const bytesOf = (s: string) => new TextEncoder().encode(s).length;

/**
 * Every object's full label, index `idx`: seats from their rows' rules and
 * `LBLX`, then the non-seat objects of `OBJS` by their stored label. Throws
 * `ChartError` for bytes that are not a chart this reader understands.
 */
export function chartLabels(file: Uint8Array): string[] {
  const view = new DataView(file.buffer, file.byteOffset, file.byteLength);
  const head = new Cur(view, 0, file.length);
  if (file.length < 32 || String.fromCharCode(...file.subarray(0, 4)) !== "CCF1") {
    throw new ChartError("magic");
  }
  head.at = 4;
  if (head.u16() !== 1) throw new ChartError("version");
  head.u16();
  const count = head.u32();
  if (head.u32() !== file.length) throw new ChartError("length");
  if (32 + count * 16 > file.length) throw new ChartError("directory");
  const chunks = new Map<string, Cur>();
  head.at = 32;
  for (let i = 0; i < count; i++) {
    const tag = String.fromCharCode(head.u8(), head.u8(), head.u8(), head.u8());
    head.at += 4;
    const offset = head.u32();
    const length = head.u32();
    if (offset + length > file.length) throw new ChartError("chunk bounds");
    if (!chunks.has(tag)) chunks.set(tag, new Cur(view, offset, offset + length));
  }
  const chunk = (tag: string): Cur => {
    const c = chunks.get(tag);
    if (c === undefined) throw new ChartError(`missing ${tag}`);
    return new Cur(view, c.at, c.end);
  };
  /** A Raw chunk: its count, checked against the bytes, and a cursor at
   * the first record. */
  const records = (tag: keyof typeof SIZES): [number, Cur] => {
    const c = chunk(tag);
    const n = c.varint();
    if (n * SIZES[tag] !== c.end - c.at) throw new ChartError(`${tag} size`);
    return [n, c];
  };

  // Strings: count, u32 end offsets, then the text.
  const s = chunk("STRS");
  const nStrings = s.varint();
  if (nStrings * 4 > s.end - s.at) throw new ChartError("STRS size");
  const ends: number[] = [];
  for (let i = 0; i < nStrings; i++) ends.push(s.u32());
  const textStart = s.at;
  const textLen = s.end - textStart;
  const strings: string[] = [];
  let prev = 0;
  for (const end of ends) {
    if (end < prev || end > textLen) throw new ChartError("STRS offsets");
    try {
      strings.push(utf8.decode(file.subarray(textStart + prev, textStart + end)));
    } catch {
      throw new ChartError("STRS text");
    }
    prev = end;
  }
  const text = (i: number): string => {
    if (i === NONE) return "";
    const t = strings[i];
    if (t === undefined) throw new ChartError("string index");
    return t;
  };

  const [metaCount, meta] = records("META");
  if (metaCount !== 1) throw new ChartError("META count");
  meta.u32();
  const template = text(meta.u32());
  meta.at += 8;
  const seatCount = meta.u32();
  if (seatCount > MAX_OBJECTS) throw new ChartError("seat count");

  const [nSections, sect] = records("SECT");
  const sections: string[] = [];
  for (let i = 0; i < nSections; i++) {
    const start = sect.at;
    sections.push(text(sect.u32()));
    sect.at = start + SIZES.SECT;
  }

  const [nRules, lblr] = records("LBLR");
  const rules: Rule[] = [];
  for (let i = 0; i < nRules; i++) {
    rules.push({
      scheme: lblr.u8(),
      overflow: lblr.u8(),
      start: lblr.u32(),
      step: lblr.u32(),
      prefix: lblr.u32(),
      suffix: lblr.u32(),
      customFirst: lblr.u32(),
      customCount: lblr.u32(),
    });
  }

  const exceptions = new Map<number, number>();
  const x = chunk("LBLX");
  const nX = x.varint();
  if (nX > MAX_OBJECTS) throw new ChartError("LBLX count");
  for (let i = 0; i < nX; i++) exceptions.set(x.varint(), x.varint());

  const [nRows, rowsCur] = records("ROWS");
  const rows: Row[] = [];
  for (let i = 0; i < nRows; i++) {
    const start = rowsCur.at;
    rows.push({
      section: rowsCur.u32(),
      label: rowsCur.u32(),
      first: rowsCur.u32(),
      seats: rowsCur.u16(),
      positions: rowsCur.u16(),
      rule: rowsCur.u32(),
      base: rowsCur.u32(),
      order: rowsCur.u8(),
    });
    rowsCur.at = start + SIZES.ROWS;
  }

  const labels: string[] = Array.from({ length: seatCount }, () => "");
  const [nSegs, seg] = records("RSEG");
  let covered = 0;
  for (let i = 0; i < nSegs; i++) {
    const row = rows[seg.u32()];
    const first = seg.u32();
    const n = seg.u16();
    const position = seg.u16();
    if (row === undefined) throw new ChartError("RSEG row");
    if (
      first !== covered ||
      first < row.first ||
      first + n > row.first + row.seats ||
      first + n > seatCount
    ) {
      throw new ChartError("RSEG partition");
    }
    covered += n;
    const section = row.section === NONE ? "" : (sections[row.section] ?? "");
    const rowLabel = text(row.label);
    for (let k = 0; k < n; k++) {
      const idx = first + k;
      const exception = exceptions.get(idx);
      let seat: string | null;
      if (exception !== undefined) {
        seat = text(exception);
      } else {
        const rule = rules[row.rule];
        if (rule === undefined) throw new ChartError("unlabelled seat");
        const ordinal = row.base + seatOrdinal(row.order, row.positions, position + k);
        seat = ruleLabel(rule, ordinal, text);
      }
      if (seat === null) throw new ChartError("seat label");
      const full = compose(template, section, rowLabel, seat);
      if (bytesOf(full) > MAX_LABEL) throw new ChartError("label length");
      labels[idx] = full;
    }
  }
  if (covered !== seatCount) throw new ChartError("RSEG partition");

  const [nObjects, objs] = records("OBJS");
  if (seatCount + nObjects > MAX_OBJECTS) throw new ChartError("object count");
  for (let i = 0; i < nObjects; i++) {
    const start = objs.at;
    objs.u8();
    labels.push(text(objs.u32()));
    objs.at = start + SIZES.OBJS;
  }
  return labels;
}

/** The label a rule gives `ordinal`, prefix and suffix included. */
function ruleLabel(rule: Rule, ordinal: number, text: (i: number) => string): string | null {
  let body: string | null;
  if (rule.scheme === 6) {
    body = ordinal < rule.customCount ? text(rule.customFirst + ordinal) : null;
  } else {
    if (rule.step === 0) return null;
    const odd = rule.scheme === 1;
    const parity = rule.scheme === 1 || rule.scheme === 2;
    if (parity && (rule.start % 2 === 1) !== odd) return null;
    const stride = parity ? rule.step * 2 : rule.step;
    const value = rule.start + stride * ordinal;
    if (value > 0xffff_ffff) return null;
    body = written(rule.scheme, rule.overflow, value);
  }
  return body === null ? null : text(rule.prefix) + body + text(rule.suffix);
}
