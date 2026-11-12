// The signed-in person (09 §2, §10): the session loaded from `me` at start,
// sign-in, setup, sign-out and password change, and the selected workspace.
// A `401` that ends the session anywhere lands here through the request
// layer; the console's frame then sends the person to sign-in and back.
import { createMemo, createRoot, createSignal } from "solid-js";
import { ApiError, call, callWithHeaders, configure } from "./api/client.ts";
import type { ConsoleLicence, ConsoleMe, ConsoleTotpSecret } from "./api/generated.ts";
import { checkChallenge, checkMe, checkRecoveryCodes, checkTotpSecret } from "./api/check.ts";
import { followLicence, type MeWorkspace } from "./api/permissions.ts";
import { asApiError } from "./api/query.ts";
import { load as loadStored, save } from "./storage.ts";

export type Session =
  | { kind: "loading" }
  /** `me` could not be loaded: the server is down or answered garbage. */
  | { kind: "failed"; error: ApiError }
  /** `cause`: no session at start (`none`), one that ended under the
   * person (`ended`: expired, or ended elsewhere), or a sign-out (`left`). */
  | { kind: "signedOut"; cause: "none" | "ended" | "left" }
  | { kind: "signedIn"; me: ConsoleMe };

const [session, setSession] = createSignal<Session>({ kind: "loading" });
export { session };

/** The signed-in person's `me`, or `null`. */
export const me = (): ConsoleMe | null => {
  const s = session();
  return s.kind === "signedIn" ? s.me : null;
};

// The selected workspace: one per browser, a convenience (not a secret).
const WORKSPACE_KEY = "cavea.workspace.v1";
const [chosen, setChosen] = createSignal<string | null>(
  loadStored(WORKSPACE_KEY, (v) => (typeof v === "string" ? v : undefined), null),
);

/** The workspace the console acts in: the chosen one if the person is
 * still a member of it, else their first; `null` with none. */
export const workspace = createRoot(() =>
  createMemo<MeWorkspace | null>(() => {
    const s = session();
    if (s.kind !== "signedIn") return null;
    const list = s.me.workspaces;
    return list.find((w) => w.id === chosen()) ?? list[0] ?? null;
  }),
);

export function chooseWorkspace(id: string): void {
  setChosen(id);
  save(WORKSPACE_KEY, id);
}

// While the licence is expired, chart and event writes are closed (12 §7.6).
followLicence(() => me()?.licence.read_only === true);

configure({
  workspace: () => workspace()?.id ?? null,
  onSessionLost: () => {
    if (session().kind === "signedIn") end("ended");
  },
});

function end(cause: "none" | "ended" | "left"): void {
  settle();
  configure({ csrf: null });
  setSession({ kind: "signedOut", cause });
}

/** Takes a `me` answer (from `me`, sign-in, setup or a password change),
 * once it is checked to be one. */
function adopt(answer: unknown): void {
  const valid = checkMe(answer);
  configure({ csrf: valid.csrf_token });
  setSession({ kind: "signedIn", me: valid });
}

let loader: AbortController | null = null;
let checked = 0;

/** Cancels a `me` load in flight: what it answers predates a sign-in,
 * sign-out or password change, and must not undo it. */
function settle(): void {
  loader?.abort();
  loader = null;
}

/** Loads the session from `me`: at start, on Retry, and when the page is
 * shown again (to notice an expired session or a changed role). */
export async function loadSession(): Promise<void> {
  loader?.abort();
  const mine = new AbortController();
  loader = mine;
  checked = Date.now();
  const quiet = session().kind === "signedIn";
  if (!quiet) setSession({ kind: "loading" });
  try {
    const answer = await call("consoleMe", {}, mine.signal);
    if (loader !== mine) return;
    adopt(answer);
  } catch (e) {
    if (loader !== mine) return;
    const error = asApiError(e);
    if (error.status === 401) {
      // Signed in a moment ago: `onSessionLost` has said it ended.
      if (session().kind !== "signedOut") end("none");
    } else if (!quiet) {
      setSession({ kind: "failed", error });
    }
  }
}

/** Re-reads `me` when the page is shown after a while away. */
export function watchSession(): void {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") return;
    if (session().kind === "signedIn" && Date.now() - checked > 60_000) void loadSession();
  });
}

/** Takes the answer of a sign-in, setup or password change. A `me` load
 * that started while it was on its way read the session before it (the old
 * CSRF token, after a password change): it is cancelled before and again
 * now, so it cannot land afterwards. */
function adoptNew(answer: unknown): void {
  settle();
  adopt(answer);
}

/** What a right password leads to: a session, or (D-89) a code to ask
 * for, with the challenge that ties it to the password. The challenge is a
 * secret: it lives in the page's memory until it is used or dropped. */
export type Signed = { kind: "signedIn" } | { kind: "code"; challenge: string };

/** Signs in, or answers that a code is needed (`202`). */
export async function signIn(
  email: string,
  password: string,
  signal: AbortSignal,
): Promise<Signed> {
  settle();
  const { reply, status } = await callWithHeaders(
    "consoleSignIn",
    { body: { email, password } },
    signal,
  );
  return signedOr(reply, status);
}

function signedOr(reply: unknown, status: number): Signed {
  if (status === 202) return { kind: "code", challenge: checkChallenge(reply).challenge };
  adoptNew(reply);
  return { kind: "signedIn" };
}

/** Sign-in's second step: the challenge and a code from the app or a
 * recovery code. */
export async function signInWithCode(challenge: string, code: string, signal: AbortSignal) {
  settle();
  adoptNew(await call("consoleSignInSecondFactor", { body: { challenge, code } }, signal));
}

/** Uses a reset link (D-90): the new password is set, and the person is
 * signed in, or asked for a code when they still have a second factor. */
export async function useResetLink(
  token: string,
  password: string,
  signal: AbortSignal,
): Promise<Signed> {
  settle();
  const { reply, status } = await callWithHeaders(
    "consolePasswordReset",
    { body: { token, password } },
    signal,
  );
  return signedOr(reply, status);
}

/** Starts turning the second factor on: its secret, shown once. Nothing is
 * on until `confirmTotp`; starting again replaces the secret. */
export async function startTotp(password: string, signal: AbortSignal): Promise<ConsoleTotpSecret> {
  return checkTotpSecret(await call("consoleTotpEnrol", { body: { password } }, signal));
}

/** Turns the second factor on with a current code: the recovery codes,
 * shown once. */
export async function confirmTotp(code: string, signal: AbortSignal): Promise<string[]> {
  const codes = checkRecoveryCodes(await call("consoleTotpConfirm", { body: { code } }, signal));
  adoptSecondFactor(true);
  return codes;
}

/** Turns the second factor off; the session stays. */
export async function disableTotp(password: string, code: string, signal: AbortSignal) {
  settle();
  adoptNew(await call("consoleTotpDisable", { body: { password, code } }, signal));
}

/** Shows whether signing in asks for a code, once it changed (or the
 * server said it already had). */
export function adoptSecondFactor(on: boolean): void {
  const s = session();
  if (s.kind !== "signedIn") return;
  setSession({ kind: "signedIn", me: { ...s.me, user: { ...s.me.user, second_factor: on } } });
}

export async function setUp(token: string, email: string, password: string, signal: AbortSignal) {
  settle();
  adoptNew(await call("consoleSetup", { body: { token, email, password } }, signal));
}

export async function changePassword(current: string, next: string, signal: AbortSignal) {
  settle();
  adoptNew(
    await call(
      "consolePassword",
      { body: { current_password: current, new_password: next } },
      signal,
    ),
  );
}

export async function signOut(signal: AbortSignal): Promise<void> {
  settle();
  try {
    await call("consoleSignOut", {}, signal);
  } catch (e) {
    // Already ended on the server: signed out all the same.
    if (!(e instanceof ApiError && e.status === 401)) throw e;
  }
  end("left");
}

/** Accepts an invitation (09 §7): signed in, as the session's user (no
 * password); otherwise as a new account with `password`, signed in by the
 * answer. The joined workspace becomes the selected one. */
export async function acceptInvitation(
  token: string,
  password: string | undefined,
  signal: AbortSignal,
): Promise<void> {
  const before = new Set(me()?.workspaces.map((w) => w.id) ?? []);
  settle();
  const body = password === undefined ? { token } : { token, password };
  const answer = checkMe(await call("acceptInvitation", { body }, signal));
  const joined = answer.workspaces.find((w) => !before.has(w.id));
  if (joined !== undefined) chooseWorkspace(joined.id);
  adoptNew(answer);
}

/** Shows a workspace's new name at once, after a rename. */
export function adoptWorkspaceName(id: string, name: string): void {
  const s = session();
  if (s.kind !== "signedIn") return;
  const workspaces = s.me.workspaces.map((w) => (w.id === id ? { ...w, name } : w));
  setSession({ kind: "signedIn", me: { ...s.me, workspaces } });
}

/** Shows a new licence state at once: after an upload, or a refusal that
 * says the licence has expired. */
export function adoptLicence(licence: ConsoleLicence): void {
  const s = session();
  if (s.kind !== "signedIn") return;
  setSession({ kind: "signedIn", me: { ...s.me, licence } });
}
