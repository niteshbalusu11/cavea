// The text field keeps what the edit core keeps (03 §2.7: at most 256
// bytes of UTF-8), cut on a character, never inside one.
import { expect, test } from "bun:test";
import { fit } from "./textLimit.ts";

test("a text longer than 256 bytes is cut to what a text holds, on a character", () => {
  expect(fit("STAGE")).toBe("STAGE");
  expect(fit("a".repeat(300))).toBe("a".repeat(256));
  // "é" is two bytes: 128 of them fill 256; "€" is three: 85 fit in 255.
  expect(fit("é".repeat(200))).toBe("é".repeat(128));
  expect(fit("€".repeat(100))).toBe("€".repeat(85));
  expect(new TextEncoder().encode(fit("a€".repeat(100))).length).toBeLessThanOrEqual(256);
});
