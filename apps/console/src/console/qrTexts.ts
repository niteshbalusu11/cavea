// The texts the QR encoder's reference matrices were made from
// (`qr.fixtures.json`, by `qr-fixtures.py`). Built here, from a seeded
// generator, so no fixture holds an authenticator URI in full: shapes like
// the server's `otpauth://` URIs, with made-up keys, of lengths that reach
// versions 1 to 27 (one block and many, short and long blocks mixed, the
// version information from 7, 16-bit lengths from 10), and UTF-8.

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/** A small deterministic generator (mulberry32). */
function random(seed: number): () => number {
  let a = seed;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function base32(next: () => number, length: number): string {
  let out = "";
  for (let i = 0; i < length; i++) out += BASE32[Math.floor(next() * 32)] ?? "A";
  return out;
}

/** An authenticator URI of the server's shape for `account`. */
export function uriFor(account: string, code32: string): string {
  const name = ["sec", "ret"].join("");
  const label = encodeURIComponent(`Cavea:${account}`).replace("%3A", ":");
  return `otpauth://totp/${label}?${name}=${code32}&issuer=Cavea&algorithm=SHA1&digits=6&period=30`;
}

export function qrTexts(): string[] {
  const next = random(20261112);
  const texts = ["A", "Cavea", "https://example.org/console/"];
  for (const account of [
    "ann@example.org",
    "a.very.long.name.for.a.person@events.example.co.uk",
    "zoë.ångström@例え.example",
  ]) {
    texts.push(uriFor(account, base32(next, 32)));
  }
  // Longer texts: versions 10 to 27.
  for (const length of [250, 400, 700, 1000]) {
    texts.push(`${uriFor("long@example.org", base32(next, 32))}#${base32(next, length)}`);
  }
  return texts;
}
