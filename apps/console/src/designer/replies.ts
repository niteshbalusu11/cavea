// The server's answers to a save and to the editing lock, as what the
// designer does next (08 §6.2, D-73). No DOM, so it is tested alone.
import type { ApiError } from "../api/client.ts";
import type { ChartLock } from "../api/generated.ts";
import { asApiError } from "../api/query.ts";
import type { LockInfo, LockReply, SaveReply } from "./autosave.ts";

/** Statuses a proxy answers for a server that is down or restarting. */
const UNREACHABLE = new Set([502, 503, 504]);

/** No answer, or the server cannot take it now: tried again later. */
function unreachable(error: ApiError): boolean {
  return (
    error.code === "network" ||
    error.code === "timeout" ||
    error.code === "rate_limited" ||
    error.code === "overloaded" ||
    error.code === "unavailable" ||
    error.code === "internal_error" ||
    (error.code === "unexpected" && UNREACHABLE.has(error.status))
  );
}

/** The lock a `409 chart_locked` names (`details.lock`), if it is one. */
export function lockOf(error: ApiError): LockInfo | null {
  if (error.code !== "chart_locked") return null;
  const l = error.details["lock"];
  if (typeof l !== "object" || l === null) return null;
  const { holder, email, since } = l as Record<string, unknown>;
  if (typeof holder !== "string" || typeof since !== "string") return null;
  return { holder, email: typeof email === "string" ? email : null, since };
}

export const infoOf = (l: ChartLock): LockInfo => ({
  holder: l.holder,
  email: l.email,
  since: l.since,
});

/** A failed save as what the autosave does next. `409` is two things told
 * apart by `code`: another holds the lock (`chart_locked`), or the draft
 * changed since the etag sent (`version_conflict`). */
export function saveReply(e: unknown, describe: (e: ApiError) => string): SaveReply {
  const error = asApiError(e);
  if (error.status === 401) return { kind: "signedOut" };
  if (error.code === "version_conflict") {
    const etag = error.details["draft_etag"];
    return { kind: "conflict", etag: typeof etag === "string" ? etag : null };
  }
  const lock = lockOf(error);
  if (lock !== null) return { kind: "locked", lock };
  if (unreachable(error)) {
    return {
      kind: "offline",
      retryAfterMs: error.retryAfter === null ? null : error.retryAfter * 1000,
    };
  }
  return { kind: "refused", message: describe(error) };
}

/** A failed lock request as what the designer does next. */
export function lockReplyOf(e: unknown): LockReply {
  const error = asApiError(e);
  if (error.status === 401) return { kind: "signedOut" };
  const lock = lockOf(error);
  if (lock !== null) return { kind: "locked", lock };
  if (unreachable(error)) return { kind: "offline" };
  return { kind: "refused" };
}
