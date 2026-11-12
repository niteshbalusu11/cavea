// The invitation link's token and a reset link's: each taken on its own
// page only, gone from the address bar, and not mistaken for a setup token
// (CON-012, CON-003).
import { afterEach, beforeEach, expect, test } from "bun:test";
import { dropInviteToken, inviteToken, takeInviteToken } from "./inviteToken.ts";
import { dropResetToken, resetToken, takeResetToken } from "./resetToken.ts";
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
  dropResetToken();
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

test("a reset link's token is the reset page's alone, and leaves the address bar", () => {
  at("/console/reset", `#token=${TOKEN}`);
  takeInviteToken();
  expect(replaced).toEqual([]);
  takeResetToken();
  expect(replaced).toEqual(["/console/reset"]);
  window.location.hash = "";
  takeSetupToken();
  expect([resetToken(), inviteToken(), setupToken()]).toEqual([TOKEN, null, null]);
  dropResetToken();
  expect(resetToken()).toBeNull();
  // Not taken on another page, nor kept when it cannot be one.
  at("/console/invite", `#token=${TOKEN}`);
  takeResetToken();
  expect(resetToken()).toBeNull();
  at("/reset", "#token=ABC");
  takeResetToken();
  expect(resetToken()).toBe("malformed");
});
