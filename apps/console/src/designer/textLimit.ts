// What a text object holds (03 §2.7), for the text field on the canvas.

/** A text holds at most this many bytes of UTF-8 (03 §2.7). */
const MAX_BYTES = 256;
const utf8 = new TextEncoder();

/** The longest start of `text` that a text can hold. */
export function fit(text: string): string {
  if (utf8.encode(text).length <= MAX_BYTES) return text;
  let out = "";
  let bytes = 0;
  for (const ch of text) {
    const n = utf8.encode(ch).length;
    if (bytes + n > MAX_BYTES) break;
    out += ch;
    bytes += n;
  }
  return out;
}
