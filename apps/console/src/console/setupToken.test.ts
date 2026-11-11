// The setup link's token: read from the fragment, then gone from the
// address bar (CON-002).
import { afterEach, expect, test } from "bun:test";
import { readSetupToken, setupToken, takeSetupToken } from "./setupToken.ts";

const TOKEN = "0123456789abcdef".repeat(4);

test("the token is read from the fragment, and only a well-formed one", () => {
  expect(readSetupToken(`#token=${TOKEN}`)).toBe(TOKEN);
  expect(readSetupToken(`token=${TOKEN}`)).toBe(TOKEN);
  expect(readSetupToken("")).toBeNull();
  expect(readSetupToken("#other=1")).toBeNull();
  expect(readSetupToken("#token=abc")).toBe("malformed");
  expect(readSetupToken(`#token=${TOKEN.toUpperCase()}`)).toBe("malformed");
  expect(readSetupToken(`#token=${TOKEN}<script>`)).toBe("malformed");
});

const saved = globalThis.window;
afterEach(() => {
  globalThis.window = saved;
});

test("taking the token removes it from the address bar and keeps the path", () => {
  const replaced: string[] = [];
  globalThis.window = {
    location: { hash: `#token=${TOKEN}`, pathname: "/console/setup", search: "?a=1" },
    history: {
      state: null,
      replaceState: (_s: unknown, _t: string, url: string) => replaced.push(url),
    },
  } as unknown as typeof window;
  takeSetupToken();
  expect(setupToken()).toBe(TOKEN);
  expect(replaced).toEqual(["/console/setup?a=1"]);
});
