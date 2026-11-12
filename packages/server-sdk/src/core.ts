// The request layer: every call goes through `transport`'s `send`, which
// turns one operation of the generated table (`OPERATIONS`) and its typed
// input into an HTTP request and its answer into the typed reply or a
// `CaveaError`. The rules a backend must follow live here, once:
//
// - the credential the operation takes, as `Authorization: Bearer`, never in
//   a URL; the keys stay inside the closure, out of every object a caller
//   can print;
// - path parameters percent-encoded, query parameters (an array repeats);
// - `Idempotency-Key` on the seat commands that take one (08 §6.1): the
//   caller's, or one made for the logical call, the same on every retry;
// - a timeout per attempt, and the caller's `AbortSignal` over the whole call;
// - retries only where repeating cannot do something twice (below).
import { CaveaError, problem } from "./errors.ts";
import { OPERATIONS, type OperationId, type Operations } from "./generated.ts";

export type Input<Op extends OperationId> = Operations[Op]["input"];
export type Reply<Op extends OperationId> = Operations[Op]["reply"];

/** Per call. */
export interface CallOptions {
  /** Cancels the call, retries and waits included; it rejects with the
   * signal's reason. */
  signal?: AbortSignal;
  /** How long one attempt may take, in milliseconds (the client's
   * `timeoutMs` by default). */
  timeoutMs?: number;
  /** Attempts after the first, where a retry is safe (the client's
   * `maxRetries` by default). */
  maxRetries?: number;
  /** For the operations that take one (hold, release, book, change status,
   * extending and releasing a hold token): 1 to 255 printable ASCII
   * characters, such as your order number. Without it the client makes one
   * per call. Send the same key again only with the same request. */
  idempotencyKey?: string;
  /** Your own request id, sent as `Cavea-Request-Id` to correlate with your
   * logs: 1 to 64 characters of `A-Z a-z 0-9 . _ -` (anything else is not
   * sent). */
  requestId?: string;
}

/** A reply with the answer's status and headers (a draft's `ETag`). */
export interface Answer<T> {
  reply: T;
  status: number;
  headers: Headers;
}

export type Fetch = (input: string, init: RequestInit) => Promise<Response>;

export interface TransportOptions {
  baseUrl: string;
  secretKey: string | undefined;
  adminKey: string | undefined;
  fetch: Fetch;
  timeoutMs: number;
  maxRetries: number;
  /** A `Retry-After` longer than this, in seconds, is not waited for: the
   * call fails with it. */
  maxRetryAfter: number;
  /** The first back-off delay without `Retry-After`, in milliseconds. */
  backoffMs: number;
}

export type Send = <Op extends OperationId>(
  op: Op,
  input: Input<Op>,
  options?: CallOptions,
) => Promise<Answer<Reply<Op>>>;

interface Parts {
  path?: Record<string, string>;
  query?: Record<string, string | number | boolean | readonly string[] | undefined>;
  headers?: Record<string, string | undefined>;
  body?: unknown;
}

/** Printable ASCII, 1 to 255 characters (08 §6.1), without a space at either
 * end: a header value loses those, so two keys would become one. */
const IDEMPOTENCY_KEY = /^[\x21-\x7e](?:[\x20-\x7e]{0,253}[\x21-\x7e])?$/;
const REQUEST_ID = /^[A-Za-z0-9._-]{1,64}$/;
/** Statuses a retry may cure: rate limited (08 §12), and the server or a
 * proxy in front of it failing or overloaded. */
const TRANSIENT = new Set([429, 500, 502, 503, 504]);

/** The URL of `template` with `parts`, after `base`. */
function target(base: string, template: string, parts: Parts): string {
  const path = template.replace(/\{(\w+)\}/g, (_, name: string) => {
    const value = parts.path?.[name];
    if (typeof value !== "string" || value === "") {
      throw new TypeError(`missing path parameter ${name}`);
    }
    return encodeURIComponent(value);
  });
  const query = new URLSearchParams();
  for (const [name, value] of Object.entries(parts.query ?? {})) {
    if (Array.isArray(value)) {
      for (const v of value) {
        query.append(name, String(v));
      }
    } else if (value !== undefined) {
      query.set(name, String(value));
    }
  }
  const search = query.toString();
  return `${base}${path}${search === "" ? "" : `?${search}`}`;
}

const isJson = (response: Response): boolean =>
  /^application\/([\w.+-]+\+)?json\b/i.test(response.headers.get("Content-Type") ?? "");

/**
 * Whether repeating `op` cannot do anything twice: a read; an archive or a
 * revocation (`DELETE`, idempotent by the API, 08 §6.2); a draft saved over
 * `If-Match: *`; or a call carrying an `Idempotency-Key`, which the server
 * replays (08 §6.1). Never a create, a publish, an event change or a draft
 * saved over an etag (its repeat would be refused as a conflict with
 * itself).
 */
function retrySafe(op: OperationId, parts: Parts): boolean {
  const info = OPERATIONS[op];
  switch (info.method) {
    case "GET":
    case "DELETE":
      return true;
    case "PUT":
      return parts.headers?.["If-Match"] === "*";
    default:
      return info.idempotencyKey;
  }
}

/** Waits `ms`, or rejects with `signal`'s reason. */
function sleep(ms: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    const done = (): void => {
      signal?.removeEventListener("abort", stop);
      resolve();
    };
    const timer = setTimeout(done, ms);
    const stop = (): void => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    signal?.addEventListener("abort", stop, { once: true });
  });
}

/** The request layer over `options`. */
export function transport(options: TransportOptions): Send {
  const base = options.baseUrl.replace(/\/+$/, "");
  const { secretKey, adminKey } = options;

  return async function send<Op extends OperationId>(
    op: Op,
    input: Input<Op>,
    call: CallOptions = {},
  ): Promise<Answer<Reply<Op>>> {
    const info = OPERATIONS[op];
    if (info === undefined) {
      throw new TypeError(`unknown operation ${String(op)}`);
    }
    const key = info.credential === "admin" ? adminKey : secretKey;
    if (key === undefined) {
      throw new TypeError(
        `${op} needs ${info.credential === "admin" ? "the admin key (adminKey)" : "a secret key (secretKey)"}, which this client was not given`,
      );
    }
    const parts = input as Parts;
    const url = target(base, info.path, parts);
    const headers = new Headers({ Accept: "application/json" });
    for (const [name, value] of Object.entries(parts.headers ?? {})) {
      if (value !== undefined) {
        headers.set(name, value);
      }
    }
    headers.set("Authorization", `Bearer ${key}`);
    if (call.requestId !== undefined && REQUEST_ID.test(call.requestId)) {
      headers.set("Cavea-Request-Id", call.requestId);
    }
    if (info.idempotencyKey) {
      const idempotencyKey = call.idempotencyKey ?? crypto.randomUUID();
      if (!IDEMPOTENCY_KEY.test(idempotencyKey)) {
        throw new TypeError(
          "idempotencyKey must be 1 to 255 printable ASCII characters, without a space at either end",
        );
      }
      headers.set("Idempotency-Key", idempotencyKey);
    } else if (call.idempotencyKey !== undefined) {
      throw new TypeError(`${op} takes no idempotency key`);
    }
    let body: BodyInit | undefined;
    if (parts.body instanceof Blob) {
      headers.set("Content-Type", "application/octet-stream");
      body = parts.body;
    } else if (parts.body !== undefined) {
      headers.set("Content-Type", "application/json");
      body = JSON.stringify(parts.body);
    }

    const retries = retrySafe(op, parts) ? Math.max(0, call.maxRetries ?? options.maxRetries) : 0;
    const timeoutMs = call.timeoutMs ?? options.timeoutMs;
    for (let attempt = 0; ; attempt++) {
      const failed = (error: CaveaError): CaveaError | number => {
        if (attempt >= retries) {
          return error;
        }
        if (error.retryAfter !== null) {
          return error.retryAfter > options.maxRetryAfter ? error : error.retryAfter * 1000;
        }
        // Full jitter: between nothing and twice the last delay.
        return Math.random() * options.backoffMs * 2 ** attempt;
      };
      const timeout = AbortSignal.timeout(timeoutMs);
      const signal = call.signal === undefined ? timeout : AbortSignal.any([call.signal, timeout]);
      let outcome: Answer<Reply<Op>> | CaveaError;
      try {
        const response = await options.fetch(url, {
          method: info.method,
          headers,
          ...(body === undefined ? {} : { body }),
          redirect: "error",
          signal,
        });
        if (response.ok) {
          return {
            reply: (await reply(op, response)) as Reply<Op>,
            status: response.status,
            headers: response.headers,
          };
        }
        const text = await response.text();
        const error = problem(op, response, text);
        if (!TRANSIENT.has(response.status)) {
          throw error;
        }
        outcome = error;
      } catch (e) {
        if (e instanceof CaveaError) {
          throw e;
        }
        if (call.signal?.aborted) {
          throw call.signal.reason;
        }
        const timedOut = timeout.aborted;
        outcome = new CaveaError({
          operation: op,
          status: 0,
          code: timedOut ? "timeout" : "network",
          message: timedOut
            ? `${op}: no answer within ${timeoutMs} ms`
            : `${op}: the server could not be reached${causeCode(e)}`,
        });
      }
      const next = failed(outcome);
      if (typeof next !== "number") {
        throw next;
      }
      await sleep(next, call.signal);
    }
  };
}

/**
 * A success's body as the operation answers it: JSON parsed, text, or bytes;
 * `undefined` for an empty answer where the operation may give one. Anything
 * else, such as a proxy's page or nothing where a reply is due, is
 * `unexpected`: a booking must never look done when it did not reach Cavea.
 */
async function reply(op: OperationId, response: Response): Promise<unknown> {
  const info = OPERATIONS[op];
  const unexpected = (): CaveaError =>
    new CaveaError({
      operation: op,
      status: response.status,
      code: "unexpected",
      requestId: response.headers.get("Cavea-Request-Id"),
      message: `${op}: HTTP ${response.status} without the reply the operation answers`,
    });
  if (info.reply === "bytes") {
    const bytes = await response.arrayBuffer();
    return bytes.byteLength === 0 && info.empty ? undefined : bytes;
  }
  const text = await response.text();
  if (text === "" && info.empty) {
    return undefined;
  }
  if (info.reply === "text") {
    return text;
  }
  if (!isJson(response)) {
    throw unexpected();
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw unexpected();
  }
}

/** A network failure's system code (`ECONNREFUSED`), never its message,
 * which may hold the URL. */
function causeCode(error: unknown): string {
  const cause = error instanceof Error ? error.cause : undefined;
  const code =
    typeof cause === "object" && cause !== null && "code" in cause ? cause.code : undefined;
  return typeof code === "string" && /^[A-Z0-9_]{1,40}$/.test(code) ? ` (${code})` : "";
}
