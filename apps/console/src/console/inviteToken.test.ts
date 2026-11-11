// The invitation link's token: taken on the invitation page only, gone
// from the address bar, and not mistaken for a setup token (CON-012).
import { afterEach, beforeEach, expect, test } from "bun:test";
import { dropInviteToken, inviteToken, takeInviteToken } from "./inviteToken.ts";
import { dropSetupToken, setupToken, takeSetupToken } from "./setupToken.ts";

const TOKEN = "0123456789abcdef".repeat(4);
const saved = globalThis.window;
let replaced: string[] = [];

function at(pathname: string, hash: string) {
  replaced = [];
  globalThis.window = {
    location: { hash, pathname, search: "" },
    history: {
      state: null,
      replaceState: (_s: unknown, _t: string, url: string) => replaced.push(url),
    },
  } as unknown as Window & typeof globalThis;
}

beforeEach(() => {
  dropInviteToken();
  dropSetupToken();
});
afterEach(() => {
  globalThis.window = saved;
});

test("on the invitation page the token is taken, not as a setup token", () => {
  at("/console/invite", `#token=${TOKEN}`);
  takeInviteToken();
  // The address bar loses it before the setup page's reader runs.
  expect(replaced).toEqual(["/console/invite"]);
  window.location.hash = "";
  takeSetupToken();
  expect(inviteToken()).toBe(TOKEN);
  expect(setupToken()).toBeNull();
});

test("elsewhere the invitation reader leaves the fragment alone", () => {
  at("/console/setup", `#token=${TOKEN}`);
  takeInviteToken();
  expect(replaced).toEqual([]);
  expect(inviteToken()).toBeNull();
});

test("a malformed token is kept as such, and a used one is dropped", () => {
  at("/invite/", "#token=abc");
  takeInviteToken();
  expect(inviteToken()).toBe("malformed");
  expect(replaced).toEqual(["/invite/"]);
  at("/invite", `#token=${TOKEN}`);
  takeInviteToken();
  expect(inviteToken()).toBe(TOKEN);
  dropInviteToken();
  expect(inviteToken()).toBeNull();
});
