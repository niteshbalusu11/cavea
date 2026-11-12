// The QR encoder against matrices other encoders made (`qr-fixtures.py`):
// every module the same as `qrcode`'s, for versions 1 to 28 and each of the
// eight masks, and the mask the standard's penalty chooses as Nayuki's
// `qrcodegen` chooses it.
import { expect, test } from "bun:test";
import fixtures from "./qr.fixtures.json";
import { encodeQr, qrPath } from "./qr.ts";
import { qrTexts } from "./qrTexts.ts";

interface Fixture {
  text: number;
  forced: boolean;
  version: number;
  mask: number;
  rows: string[];
}

const texts = qrTexts();

/** Our matrix as the fixture's hex rows. */
function rows(modules: boolean[], size: number): string[] {
  const out: string[] = [];
  for (let y = 0; y < size; y++) {
    let bits = "";
    for (let x = 0; x < size; x++) bits += modules[y * size + x] === true ? "1" : "0";
    bits += "0".repeat((4 - (bits.length % 4)) % 4);
    let hex = "";
    for (let i = 0; i < bits.length; i += 4) hex += parseInt(bits.slice(i, i + 4), 2).toString(16);
    out.push(hex);
  }
  return out;
}

test("every reference matrix is drawn module for module", () => {
  const list = fixtures as Fixture[];
  expect(list.length).toBeGreaterThan(10);
  for (const f of list) {
    const text = texts[f.text];
    if (text === undefined) throw new Error(`no text ${f.text}`);
    const code = encodeQr(text, f.forced ? f.mask : undefined);
    expect(code).not.toBeNull();
    if (code === null) continue;
    const where = `text ${f.text}, mask ${f.mask}${f.forced ? "" : " (chosen)"}`;
    expect({ where, version: code.version, mask: code.mask }).toEqual({
      where,
      version: f.version,
      mask: f.mask,
    });
    expect({ where, rows: rows(code.modules, code.size) }).toEqual({ where, rows: f.rows });
  }
});

test("a text too long for any version is null, not an exception; odd input is drawn", () => {
  expect(encodeQr("x".repeat(2400))).toBeNull();
  // Version 40 at level M holds 2,331 bytes in byte mode.
  expect(encodeQr("x".repeat(2331))?.version).toBe(40);
  expect(encodeQr("x".repeat(2332))).toBeNull();
  expect(encodeQr("")?.version).toBe(1);
  expect(encodeQr("\u0000￿\ud800")?.size).toBe(21);
  // A mask out of range is chosen by the penalty instead.
  expect(encodeQr("Cavea", 9)?.mask).toBe(encodeQr("Cavea")?.mask);
});

test("the path draws one unit square per dark module, inside the quiet zone", () => {
  const code = encodeQr("Cavea");
  if (code === null) throw new Error("no code");
  const path = qrPath(code, 4);
  const squares = path.match(/M\d+ \d+h1v1h-1z/g) ?? [];
  expect(squares.join("")).toBe(path);
  expect(squares.length).toBe(code.modules.filter(Boolean).length);
  // The top-left finder's corner, after the four-module margin.
  expect(path.startsWith("M4 4h1v1h-1z")).toBe(true);
});
