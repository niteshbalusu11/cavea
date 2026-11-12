// The console's one request layer (09 §9, §10). Every call to the server
// goes through `call`: a relative URL on the console's own origin, the
// session cookie sent by the browser (`credentials: "same-origin"`), the CSRF
// token on every non-safe method, the selected workspace on every route that
// checks a permission, an `AbortSignal` from whoever asked, and every failure
// turned into an `ApiError` carrying the problem's code and request id.
//
// The CSRF token lives here, in memory only; the workspace is read from
// the session's selection at each request.
import { OPERATIONS, type OperationId, type Operations, type Problem } from "./generated.ts";

export type Input<Op extends OperationId> = Operations[Op]["input"];
export type Reply<Op extends OperationId> = Operations[Op]["reply"];

/** What went wrong: a problem's `code`, or one of the layer's own for a
 * request that got no usable answer. */
export type ErrorCode = Problem["code"] | "network" | "timeout" | "unexpected";

export class ApiError extends Error {
  constructor(
    /** The HTTP status, or 0 when no answer arrived. */
    readonly status: number,
    readonly code: ErrorCode,
    /** `details.reason`, when the problem gives one. */
    readonly reason: string | null,
    readonly details: Readonly<Record<string, unknown>>,
    /** `Cavea-Request-Id`, for a report; `null` when no answer arrived. */
    readonly requestId: string | null,
    /** `Retry-After`, in seconds. */
    readonly retryAfter: number | null,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }

  /** The session has ended (or there never was one): sign in again. */
  get sessionLost(): boolean {
    return this.status === 401 && (this.reason === "console_session" || this.reason === "missing");
  }
}

/** True for the rejection of a request its caller cancelled. */
export function isAbort(e: unknown): boolean {
  return e instanceof DOMException && e.name === "AbortError";
}

interface Context {
  csrf: string | null;
  /** The selected workspace's id, `ws_…`, read at each request. */
  workspace: () => string | null;
  /** Called on a `401` that ends the session, before the call rejects. */
  onSessionLost: () => void;
  fetch: (input: string, init: RequestInit) => Promise<Response>;
  /** How long a request may take before it fails with `timeout`. */
  timeoutMs: number;
}

const context: Context = {
  csrf: null,
  workspace: () => null,
  onSessionLost: () => {},
  fetch: (input, init) => window.fetch(input, init),
  timeoutMs: 30_000,
};

export function configure(patch: Partial<Context>): void {
  Object.assign(context, patch);
}

const SAFE = new Set(["GET", "HEAD", "OPTIONS"]);

interface Parts {
  path?: Record<string, string>;
  /** An array is the parameter repeated (`label=A&label=B`). */
  query?: Record<string, string | number | boolean | readonly string[] | undefined>;
  headers?: Record<string, string | undefined>;
  body?: unknown;
}

export interface CallOptions {
  /** The workspace to act in (`ws_…`) instead of the selected one: for a
   * page pinned to one workspace's object, such as the designer's chart. */
  workspace?: string;
  /** Outlives the page (`fetch`'s `keepalive`): a request sent as it is
   * left, such as releasing the designer's lock. */
  keepalive?: boolean;
}

/** Calls operation `op`; rejects with an `ApiError`, or with the abort when
 * `signal` cancels it. */
export async function call<Op extends OperationId>(
  op: Op,
  input: Input<Op>,
  signal: AbortSignal,
  options: CallOptions = {},
): Promise<Reply<Op>> {
  return (await callWithHeaders(op, input, signal, options)).reply;
}

/** `call`, with the answer's status and headers (a draft's `ETag`). */
export async function callWithHeaders<Op extends OperationId>(
  op: Op,
  input: Input<Op>,
  signal: AbortSignal,
  options: CallOptions = {},
): Promise<{ reply: Reply<Op>; status: number; headers: Headers }> {
  const { response, body: buffer } = await send(op, input, signal, options, "application/json");
  const requestId = response.headers.get("Cavea-Request-Id");
  const text = new TextDecoder().decode(buffer);
  if (!response.ok) {
    const error = problem(response, text, requestId);
    if (error.sessionLost) context.onSessionLost();
    throw error;
  }
  const meta = { status: response.status, headers: response.headers };
  if (text === "") return { reply: undefined as Reply<Op>, ...meta };
  if (!isJson(response)) return { reply: text as Reply<Op>, ...meta };
  try {
    return { reply: JSON.parse(text) as Reply<Op>, ...meta };
  } catch {
    throw unexpected(response.status, requestId);
  }
}

/** `call` for an operation whose answer is bytes (a chart, a state frame):
 * the body, or `null` for `204`; failures as `call`'s. */
export async function callBytes<Op extends OperationId>(
  op: Op,
  input: Input<Op>,
  signal: AbortSignal,
): Promise<{ bytes: Uint8Array | null; requestId: string | null }> {
  const { response, body } = await send(op, input, signal, {}, "*/*");
  const requestId = response.headers.get("Cavea-Request-Id");
  if (!response.ok) {
    const error = problem(response, new TextDecoder().decode(body), requestId);
    if (error.sessionLost) context.onSessionLost();
    throw error;
  }
  return { bytes: response.status === 204 ? null : new Uint8Array(body), requestId };
}

/** Sends `op` and reads the whole answer, or rejects as `lost` says. */
async function send<Op extends OperationId>(
  op: Op,
  input: Input<Op>,
  signal: AbortSignal,
  options: CallOptions,
  accept: string,
): Promise<{ response: Response; body: ArrayBuffer }> {
  const info = OPERATIONS[op];
  const parts = input as Parts;
  const url = target(info.path, parts);
  const headers = new Headers({ Accept: accept });
  for (const [name, value] of Object.entries(parts.headers ?? {})) {
    if (value !== undefined) headers.set(name, value);
  }
  if (!SAFE.has(info.method) && context.csrf !== null) {
    headers.set("Cavea-Csrf-Token", context.csrf);
  }
  // A route that checks a permission acts in one workspace (09 §10).
  const workspace = info.permission === null ? null : (options.workspace ?? context.workspace());
  if (workspace !== null) headers.set("Cavea-Workspace", workspace);
  let body: BodyInit | undefined;
  if (parts.body instanceof Blob) {
    // Sent as it is, typed by the blob (fetch sets `Content-Type` from it):
    // an image, or JSON already serialised (a large draft).
    body = parts.body;
  } else if (parts.body !== undefined) {
    headers.set("Content-Type", "application/json");
    body = JSON.stringify(parts.body);
  }

  const timeout = AbortSignal.timeout(context.timeoutMs);
  let response: Response;
  try {
    response = await context.fetch(url, {
      method: info.method,
      headers,
      ...(body === undefined ? {} : { body }),
      credentials: "same-origin",
      ...(options.keepalive === true ? { keepalive: true } : {}),
      signal: AbortSignal.any([signal, timeout]),
    });
  } catch (e) {
    throw lost(e, signal, timeout);
  }
  try {
    return { response, body: await response.arrayBuffer() };
  } catch (e) {
    throw lost(e, signal, timeout);
  }
}

function target(template: string, parts: Parts): string {
  const path = template.replace(/\{(\w+)\}/g, (_, name: string) => {
    const value = parts.path?.[name];
    if (value === undefined) throw new TypeError(`missing path parameter ${name}`);
    return encodeURIComponent(value);
  });
  const query = new URLSearchParams();
  for (const [name, value] of Object.entries(parts.query ?? {})) {
    if (Array.isArray(value)) for (const v of value) query.append(name, String(v));
    else if (value !== undefined) query.set(name, String(value));
  }
  const search = query.toString();
  return search === "" ? path : `${path}?${search}`;
}

function isJson(response: Response): boolean {
  const type = response.headers.get("Content-Type") ?? "";
  return /^application\/([\w.+-]+\+)?json\b/i.test(type);
}

/** The rejection of a request that got no answer: the caller's abort as it
 * is, else a timeout or a network failure. */
function lost(e: unknown, signal: AbortSignal, timeout: AbortSignal): unknown {
  if (signal.aborted) return signal.reason ?? e;
  if (timeout.aborted) {
    return new ApiError(0, "timeout", null, {}, null, null, "The server did not answer in time.");
  }
  return new ApiError(0, "network", null, {}, null, null, "The server could not be reached.");
}

function unexpected(
  status: number,
  requestId: string | null,
  retryAfter: number | null = null,
): ApiError {
  return new ApiError(
    status,
    "unexpected",
    null,
    {},
    requestId,
    retryAfter,
    `The server's answer could not be read (HTTP ${status}).`,
  );
}

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
  chart_locked: true,
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

/** A failed answer as an `ApiError`: a problem+json body when it is one (a
 * proxy's or a crashed server's page is not), with `Retry-After`. */
function problem(response: Response, text: string, headerId: string | null): ApiError {
  const retry = Number(response.headers.get("Retry-After"));
  const retryAfter = Number.isInteger(retry) && retry > 0 ? retry : null;
  let body: unknown;
  try {
    body = isJson(response) ? JSON.parse(text) : undefined;
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
    return unexpected(response.status, headerId, retryAfter);
  }
  const record = body as Record<string, unknown>;
  const details =
    typeof record["details"] === "object" &&
    record["details"] !== null &&
    !Array.isArray(record["details"])
      ? (record["details"] as Record<string, unknown>)
      : {};
  const reason = typeof details["reason"] === "string" ? details["reason"] : null;
  const requestId =
    headerId ?? (typeof record["request_id"] === "string" ? record["request_id"] : null);
  const message = typeof record["message"] === "string" ? record["message"] : "";
  return new ApiError(
    response.status,
    body.code as Problem["code"],
    reason,
    details,
    requestId,
    retryAfter,
    message,
  );
}
