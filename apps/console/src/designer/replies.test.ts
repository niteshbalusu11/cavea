// The server's answers as the designer acts on them (08 §6.2, D-73): a
// `409` is a stale draft or another's lock, told apart by `code`.
import { expect, test } from "bun:test";
import { ApiError } from "../api/client.ts";
import { lockReplyOf, saveReply } from "./replies.ts";

const saveReplyOf = (e: unknown) => saveReply(e, () => "refused");

const lock = { holder: "usr_b", email: "b@example.org", since: "2026-11-11T10:00:00Z" };
const problem = (status: number, code: ApiError["code"], details: Record<string, unknown> = {}) =>
  new ApiError(
    status,
    code,
    (details["reason"] as string | undefined) ?? null,
    details,
    "r",
    null,
    "",
  );

test("a save refused by another's lock is not a conflict, and the reverse", () => {
  const locked = problem(409, "chart_locked", { lock: { ...lock, expires_at: "…" } });
  expect(saveReplyOf(locked)).toEqual({ kind: "locked", lock });
  expect(saveReplyOf(problem(409, "version_conflict", { draft_etag: '"7"' }))).toEqual({
    kind: "conflict",
    etag: '"7"',
  });
  // A viewer's or a key's view of the lock: no email.
  const hidden = problem(409, "chart_locked", { lock: { ...lock, email: null } });
  expect(saveReplyOf(hidden)).toEqual({ kind: "locked", lock: { ...lock, email: null } });
});

test("taking the lock: held elsewhere, unreachable, signed out, or refused for good", () => {
  expect(lockReplyOf(problem(409, "chart_locked", { lock }))).toEqual({ kind: "locked", lock });
  expect(lockReplyOf(problem(0, "network"))).toEqual({ kind: "offline" });
  expect(lockReplyOf(problem(503, "unavailable"))).toEqual({ kind: "offline" });
  expect(lockReplyOf(problem(401, "unauthorized", { reason: "console_session" }))).toEqual({
    kind: "signedOut",
  });
  expect(lockReplyOf(problem(400, "invalid_request", { reason: "archived" }))).toEqual({
    kind: "refused",
  });
  // A 409 without the lock it should name is not taken for one.
  expect(lockReplyOf(problem(409, "chart_locked"))).toEqual({ kind: "refused" });
});
