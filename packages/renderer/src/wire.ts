// The few realtime frames `caveaServer` reads or writes itself (docs/08-server.md
// §8): it encodes SUBSCRIBE and PONG and reads HELLO and the `seq` of SNAPSHOT
// and DELTA. Everything else goes to the engine as bytes. Pinned to the Rust
// codecs by `wire.fixture.json` (checked in `crates/cavea-web`).

export const HELLO = 1;
export const SNAPSHOT = 2;
export const DELTA = 3;
export const PRIVATE = 4;
export const PING = 6;
export const CLOSE = 7;

const malformed = (): Error => new Error("malformed realtime frame");

/** The LEB128 at `at` and the offset after it; above 2^53 is refused. */
function varint(bytes: Uint8Array, at: number): [number, number] {
  let value = 0;
  for (let i = 0, scale = 1; i < 10; i++, scale *= 128) {
    const byte = bytes[at + i];
    if (byte === undefined) {
      break;
    }
    value += (byte & 0x7f) * scale;
    if (byte < 0x80) {
      if (Number.isSafeInteger(value)) {
        return [value, at + i + 1];
      }
      break;
    }
  }
  throw malformed();
}

function putVarint(out: number[], value: number): void {
  for (; value >= 0x80; value = Math.floor(value / 0x80)) {
    out.push((value % 0x80) | 0x80);
  }
  out.push(value);
}

/** HELLO: the negotiated version, the server's clock (Unix ms), the chart's hash in hex. */
export function readHello(frame: Uint8Array): { version: number; time: number; hash: string } {
  const [time, at] = varint(frame, 2);
  const hash = frame.subarray(at, at + 16);
  const [, end] = varint(frame, at + 16);
  if (frame[0] !== HELLO || hash.length !== 16 || end !== frame.length) {
    throw malformed();
  }
  const hex = Array.from(hash, (b) => b.toString(16).padStart(2, "0")).join("");
  return { version: frame[1] ?? 0, time, hash: hex };
}

/** A SNAPSHOT's `seq`, or a DELTA's `from_seq` and `to_seq`. */
export function readSeq(frame: Uint8Array): [from: number, to: number] {
  const [first, at] = varint(frame, 1);
  return frame[0] === DELTA ? [first, varint(frame, at)[0]] : [first, first];
}

/** SUBSCRIBE in protocol version 1; absent tokens are `null`. */
export function subscribe(
  event: string,
  lastSeq: number,
  holdToken: string | null,
  sessionToken: string | null,
): Uint8Array<ArrayBuffer> {
  const out = [0x10, 1];
  const text = (value: string): void => {
    const bytes = new TextEncoder().encode(value);
    putVarint(out, bytes.length);
    out.push(...bytes);
  };
  text(event);
  putVarint(out, lastSeq);
  for (const token of [holdToken, sessionToken]) {
    out.push(token === null ? 0 : 1);
    if (token !== null) {
      text(token);
    }
  }
  return Uint8Array.from(out);
}

/** The PONG answering `ping`: its nonce, byte for byte (a nonce may pass 2^53). */
export function pong(ping: Uint8Array): Uint8Array<ArrayBuffer> {
  const out = ping.slice();
  out[0] = 0x11;
  return out;
}

/** An SSE `data:` line's frame. */
export function fromBase64(data: string): Uint8Array {
  return Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
}
