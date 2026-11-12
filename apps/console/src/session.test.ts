// A `me` load that read the session before a sign-in, sign-out or password
// change must not land after it (it would bring back the old session or the
// old CSRF token).
import { expect, test } from "bun:test";
import { configure } from "./api/client.ts";
import { changePassword, loadSession, session, signIn, signOut } from "./session.ts";

const me = (csrf: string) => ({
  user: { id: "usr_1", email: "a@example.org", second_factor: false },
  workspaces: [],
  csrf_token: csrf,
  session: { expires_at: "2026-11-11T00:00:00Z", absolute_expires_at: "2026-11-18T00:00:00Z" },
});
const json = (body: unknown) =>
  new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });

/** Answers each request when its test says so. */
const waiting: { url: string; answer: (r: Response) => void }[] = [];
configure({
  fetch: (url) => new Promise((answer) => waiting.push({ url, answer })),
});
const next = async (path: string) => {
  for (let i = 0; i < 50 && !waiting.some((w) => w.url === path); i++) await Promise.resolve();
  const at = waiting.findIndex((w) => w.url === path);
  expect(at).toBeGreaterThanOrEqual(0);
  return waiting.splice(at, 1)[0]!;
};
const csrf = () => {
  const s = session();
  return s.kind === "signedIn" ? s.me.csrf_token : s.kind;
};
const signal = new AbortController().signal;

test("a refresh started during a password change cannot restore the old token", async () => {
  const signingIn = signIn("a@example.org", "p", signal);
  (await next("/console/api/session")).answer(json(me("old")));
  await signingIn;
  expect(csrf()).toBe("old");

  const changing = changePassword("p", "q", signal);
  const password = await next("/console/api/password");
  // The page is shown again meanwhile: a refresh reads `me` with the old cookie.
  const refreshing = loadSession();
  const stale = await next("/console/api/me");
  password.answer(json(me("new")));
  await changing;
  stale.answer(json(me("old")));
  await refreshing;
  expect(csrf()).toBe("new");
});

test("a refresh in flight at sign-out cannot sign the person back in", async () => {
  const refreshing = loadSession();
  const stale = await next("/console/api/me");
  const leaving = signOut(signal);
  (await next("/console/api/session")).answer(new Response(null, { status: 204 }));
  await leaving;
  stale.answer(json(me("new")));
  await refreshing;
  expect(csrf()).toBe("signedOut");
});
