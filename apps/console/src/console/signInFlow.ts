// Sign-in's two steps as one plain function (CON-003, D-89), so the rules
// are tested once and the sign-in and reset pages follow them alike: the
// password (or a reset link's new password), then, for an account with a
// second factor, a code tied to the password by a challenge. A wrong code
// keeps the challenge; a dead one (used, five minutes old, five wrong
// codes, the password changed) goes back to the password with a message;
// a back-off waits; nothing here says whether an email exists or has a
// second factor before the password was right. Where the person goes
// afterwards is the page's (`?next=` on sign-in, kept through both steps).
import type { ApiError } from "../api/client.ts";
import { msg } from "../i18n/messages.ts";

export interface Flow {
  step: "password" | "code";
  /** The code step's challenge: a secret, in memory only, dropped when the
   * step ends. */
  challenge: string | null;
  /** On the current step's field (the code). */
  fieldError: string | null;
  /** For the form as a whole, in words. */
  notice: string | null;
  /** A refusal to describe in general terms (with its request id). */
  failure: ApiError | null;
  /** Seconds to wait before trying again, when the server said so. */
  wait: number | null;
}

export type FlowEvent =
  /** The password was right and a code is needed. */
  | { type: "challenge"; challenge: string }
  /** The current step was refused. */
  | { type: "refused"; error: ApiError }
  /** The person chose to start again from the password. */
  | { type: "restart" }
  /** A new attempt: what the last one said is cleared. */
  | { type: "submit" };

export const START: Flow = {
  step: "password",
  challenge: null,
  fieldError: null,
  notice: null,
  failure: null,
  wait: null,
};

const quiet = { fieldError: null, notice: null, failure: null, wait: null };

/** The password step's refusal, as sign-in has always said it: one message
 * for a wrong email or password. */
function passwordRefused(flow: Flow, e: ApiError): Flow {
  if (e.status === 401 && e.reason === "credentials") {
    return { ...flow, ...quiet, notice: msg("signIn.wrong") };
  }
  return common(flow, e);
}

function codeRefused(flow: Flow, e: ApiError): Flow {
  if (e.status === 401 && e.reason === "second_factor") {
    return { ...flow, ...quiet, fieldError: msg("signIn.codeWrong") };
  }
  if (e.status === 401 && e.reason === "challenge") {
    return { ...START, notice: msg("signIn.challengeEnded") };
  }
  return common(flow, e);
}

/** What either step says alike: a back-off or the per-address budget
 * (`Retry-After`), the server busy hashing, anything else in general. */
function common(flow: Flow, e: ApiError): Flow {
  if (e.status === 429) return { ...flow, ...quiet, wait: e.retryAfter ?? 60 };
  if (e.reason === "password_hashing") return { ...flow, ...quiet, notice: msg("signIn.busy") };
  return { ...flow, ...quiet, failure: e };
}

export function next(flow: Flow, event: FlowEvent): Flow {
  switch (event.type) {
    case "challenge":
      return { ...START, step: "code", challenge: event.challenge };
    case "refused":
      return flow.step === "code"
        ? codeRefused(flow, event.error)
        : passwordRefused(flow, event.error);
    case "restart":
      return START;
    case "submit":
      return { ...flow, ...quiet };
  }
}

/** A code as typed or pasted: the server ignores spaces, dashes and case,
 * so only an empty one is refused here. */
export function codeProblem(code: string): string | null {
  return code.replace(/[\s-]/g, "") === "" ? msg("signIn.codeMissing") : null;
}
