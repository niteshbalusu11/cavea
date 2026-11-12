// Sign-in's two steps (CON-003, D-89): the password, the challenge, a wrong
// code kept on the same challenge, a dead challenge back to the password
// with a message, back-off at either step, and the way back to where the
// person was.
import { expect, test } from "bun:test";
import { ApiError, type ErrorCode } from "../api/client.ts";
import { msg } from "../i18n/messages.ts";
import { safeNext, signInFor } from "./paths.ts";
import { codeProblem, next, START, type Flow, type FlowEvent } from "./signInFlow.ts";

function refused(
  status: number,
  code: ErrorCode,
  reason: string | null,
  retryAfter: number | null = null,
): FlowEvent {
  const details = reason === null ? {} : { reason };
  return {
    type: "refused",
    error: new ApiError(status, code, reason, details, "req_1", retryAfter, "server text"),
  };
}
const run = (events: FlowEvent[], from: Flow = START) => events.reduce(next, from);
/** A made-up challenge, built at run time. */
const challenge = "c".repeat(32) + "d".repeat(32);

test("a right password with a second factor asks for a code; a wrong one says one thing", () => {
  const wrong = run([{ type: "submit" }, refused(401, "unauthorized", "credentials")]);
  expect(wrong.step).toBe("password");
  expect(wrong.notice).toBe(msg("signIn.wrong"));
  // The next attempt clears it; a right password leads to the code step.
  const asked = run([{ type: "submit" }, { type: "challenge", challenge }], wrong);
  expect(asked).toEqual({ ...START, step: "code", challenge });
});

test("a wrong or used code stays on the challenge; a dead challenge goes back to the password", () => {
  const code = run([{ type: "challenge", challenge }]);
  const wrong = run([{ type: "submit" }, refused(401, "unauthorized", "second_factor")], code);
  expect(wrong).toMatchObject({ step: "code", challenge, fieldError: msg("signIn.codeWrong") });
  const again = run([{ type: "submit" }], wrong);
  expect(again.fieldError).toBeNull();
  expect(again.challenge).toBe(challenge);
  // Used, five minutes old, five wrong codes, or the password changed.
  const dead = run([refused(401, "unauthorized", "challenge")], again);
  expect(dead).toEqual({ ...START, notice: msg("signIn.challengeEnded") });
  // Starting again drops the challenge.
  expect(run([{ type: "restart" }], code)).toEqual(START);
});

test("a back-off waits out Retry-After at either step, keeping the step", () => {
  const atPassword = run([refused(429, "rate_limited", "backoff", 17)]);
  expect(atPassword).toMatchObject({ step: "password", wait: 17 });
  const atCode = run([{ type: "challenge", challenge }, refused(429, "rate_limited", "sign_in")]);
  expect(atCode).toMatchObject({ step: "code", challenge, wait: 60 });
  expect(run([{ type: "submit" }], atCode).wait).toBeNull();
  const busy = run([refused(503, "overloaded", "password_hashing")], atCode);
  expect(busy).toMatchObject({ step: "code", notice: msg("signIn.busy") });
  // Anything else is described in general, with its request id.
  const other = run([refused(403, "forbidden", "origin")], atCode);
  expect(other.failure?.reason).toBe("origin");
  expect(other.step).toBe("code");
});

test("the way back is kept through both steps: one address, only this console's paths", () => {
  // Sign-in reads `next` from its own address, which neither step changes.
  const back = signInFor("/events/opening-night?tab=1");
  const nextParam = new URLSearchParams(back.split("?")[1]).get("next") ?? undefined;
  expect(safeNext(nextParam)).toBe("/events/opening-night?tab=1");
  for (const elsewhere of ["//evil.example", "/\\evil.example", "https://evil.example/"]) {
    expect(safeNext(elsewhere)).toBe("/charts");
  }
});

test("a code is any text but an empty one: spaces, dashes and case are the server's", () => {
  expect(codeProblem("")).toBe(msg("signIn.codeMissing"));
  expect(codeProblem(" - ")).toBe(msg("signIn.codeMissing"));
  expect(codeProblem("123 456")).toBeNull();
  expect(codeProblem("ABCDE-fghij")).toBeNull();
});
