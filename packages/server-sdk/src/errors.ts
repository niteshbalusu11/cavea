// The one error the SDK throws for a call that failed: a problem's
// `code` (08 §6.4) or one of the client's own, with `reason`, `details`, the
// request id and `Retry-After`. It never holds the key, the request's body,
// its headers, its URL (a hold token travels in some paths) or the server's
// free-text message, so printing or logging it is safe.
import type { OperationId, Problem } from "./generated.ts";

/** What went wrong: a problem's `code`, or the client's own for a call that
 * got no usable answer: `network` (no answer), `timeout` (no answer in
 * time), `unexpected` (an answer that is not a problem of the API, such as a
 * proxy's error page). */
export type ErrorCode = Problem["code"] | "network" | "timeout" | "unexpected";

/** Every problem code (typed: a code the document gains must be added). */
const CODES: Record<Problem["code"], true> = {
  invalid_request: true,
  unauthorized: true,
  forbidden: true,
  not_found: true,
  already_exists: true,
  object_not_free: true,
  object_not_held_by_token: true,
  object_status_conflict: true,
  version_conflict: true,
  upgrade_conflict: true,
  idempotency_mismatch: true,
  hold_token_expired: true,
  hold_limit_exceeded: true,
  channel_not_allowed: true,
  orphan_rule_violated: true,
  not_enough_objects: true,
  chart_invalid: true,
  rate_limited: true,
  internal_error: true,
  overloaded: true,
  unavailable: true,
};

export class CaveaError extends Error {
  override readonly name = "CaveaError";
  /** The operation called (`book`, `getChart`, …). */
  readonly operation: OperationId;
  /** The HTTP status, or 0 when no answer arrived. */
  readonly status: number;
  readonly code: ErrorCode;
  /** `details.reason`, when the problem gives one (`invalid_field`, `archived`, …). */
  readonly reason: string | null;
  /** The problem's `details`; `{}` when it has none. */
  readonly details: Readonly<Record<string, unknown>>;
  /** `Cavea-Request-Id`, to find the request in the server's logs; `null`
   * when no answer arrived. */
  readonly requestId: string | null;
  /** `Retry-After`, in seconds, when the answer gave one. */
  readonly retryAfter: number | null;

  constructor(fields: {
    operation: OperationId;
    status: number;
    code: ErrorCode;
    reason?: string | null;
    details?: Readonly<Record<string, unknown>>;
    requestId?: string | null;
    retryAfter?: number | null;
    message: string;
  }) {
    super(fields.message);
    this.operation = fields.operation;
    this.status = fields.status;
    this.code = fields.code;
    this.reason = fields.reason ?? null;
    this.details = fields.details ?? {};
    this.requestId = fields.requestId ?? null;
    this.retryAfter = fields.retryAfter ?? null;
  }
}

/** True when `error` is a `CaveaError`, with `code` if given. */
export function isCaveaError(error: unknown, code?: ErrorCode): error is CaveaError {
  return error instanceof CaveaError && (code === undefined || error.code === code);
}

/** `Retry-After` in whole seconds (08 §12 sends only those), or `null`. */
export function retryAfter(headers: Headers): number | null {
  const value = headers.get("Retry-After");
  if (value === null || !/^\d{1,6}$/.test(value.trim())) {
    return null;
  }
  return Number(value.trim());
}

/** A failed answer as a `CaveaError`: a problem+json body when it is one of
 * the API's (a proxy's or a crashed server's page is not). */
export function problem(operation: OperationId, response: Response, text: string): CaveaError {
  const requestId = response.headers.get("Cavea-Request-Id");
  const wait = retryAfter(response.headers);
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    body = undefined;
  }
  if (
    typeof body !== "object" ||
    body === null ||
    !("code" in body) ||
    typeof body.code !== "string" ||
    !Object.hasOwn(CODES, body.code)
  ) {
    return new CaveaError({
      operation,
      status: response.status,
      code: "unexpected",
      requestId,
      retryAfter: wait,
      message: `${operation}: HTTP ${response.status} without a problem of the API`,
    });
  }
  const code = body.code as Problem["code"];
  const details =
    "details" in body && typeof body.details === "object" && body.details !== null
      ? (body.details as Record<string, unknown>)
      : {};
  const reason = typeof details["reason"] === "string" ? details["reason"] : null;
  const field = typeof details["field"] === "string" ? details["field"] : null;
  const id =
    requestId ??
    ("request_id" in body && typeof body.request_id === "string" ? body.request_id : null);
  // The problem's `message` is left out: the server's text for a refused body
  // can quote part of it (a value of the wrong type).
  const what = [code, reason, field === null ? null : `field ${field}`].filter((s) => s !== null);
  return new CaveaError({
    operation,
    status: response.status,
    code,
    reason,
    details,
    requestId: id,
    retryAfter: wait,
    message: `${operation}: ${what.join(", ")} (HTTP ${response.status}${id === null ? "" : `, ${id}`})`,
  });
}
