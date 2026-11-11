// `@cavea/server-sdk`: the Cavea server's REST API for a customer's backend
// (05 §1, §15). It holds a secret key, so it refuses to run in a browser.
//
// Two layers. `cavea.call(op, input)` reaches every operation a secret key or
// the admin key may call, typed from the generated table, so a new operation
// on the server is callable as soon as the types are regenerated. Over it,
// one group per resource (`cavea.events.hold(…)`) with the API's rules built
// in: idempotency keys, `If-Match`, pages as async iterators, the draft's
// etag, created-or-existing replies.
import {
  type Answer,
  type CallOptions,
  type Fetch,
  type Input,
  type Reply,
  type Send,
  transport,
} from "./core.ts";
import type {
  Asset,
  Book,
  ChangeStatus,
  Chart,
  ChartDocument,
  CreateChart,
  CreateEvent,
  CreateHoldToken,
  CreateKey,
  CreateWorkspace,
  Event,
  EventObject,
  ExtendHoldToken,
  ExtendReply,
  Hold,
  HoldReply,
  HoldTokenCreated,
  HoldTokenState,
  InvitationCreated,
  InviteOwner,
  Key,
  KeyCreated,
  MintKey,
  OperationId,
  Release,
  SeatReply,
  UpdateEvent,
  Validation,
  Version,
  Workspace,
  WorkspaceCreated,
} from "./generated.ts";

export { CaveaError, isCaveaError, type ErrorCode } from "./errors.ts";
export type { Answer, CallOptions, Fetch, Input, Reply } from "./core.ts";
export type * from "./generated.ts";

export interface CaveaOptions {
  /** The server's base URL, such as `https://seats.example.com` (a path
   * prefix is kept). */
  baseUrl: string;
  /** A workspace's secret key, `sk_…`: every workspace operation. */
  secretKey?: string;
  /** The id of `secretKey` (`key_…`, from `keys.list()` or its creation):
   * needed only to sign session tokens. */
  keyId?: string;
  /** The operator's `CAVEA_ADMIN_KEY`: `workspaces` and `metrics` only. */
  adminKey?: string;
  /** How long one attempt may take, in milliseconds. Default 30,000. */
  timeoutMs?: number;
  /** Retries after the first attempt, where a retry is safe. Default 2. */
  maxRetries?: number;
  /** The longest `Retry-After` waited for, in seconds; longer fails the call
   * with it. Default 30. */
  maxRetryAfter?: number;
  /** The `fetch` to use; the platform's by default. */
  fetch?: Fetch;
  /** Run where `window` and `document` exist. Only for tests that fake a
   * browser: a page that holds a secret key gives it to every visitor. */
  dangerouslyAllowBrowser?: boolean;
}

/** A page of a list. */
export interface Page<T> {
  data: T[];
  /** Pass back as `after`; `null` on the last page. */
  next: string | null;
}

/** A list, walked page by page as it is iterated: every item once, in the
 * server's order. */
export interface Paginated<T> extends AsyncIterable<T> {
  /** The pages themselves; a page may be empty and still not be the last
   * (an objects page looks at 10,000 objects at most, 08 §6.2). */
  pages(): AsyncIterable<Page<T>>;
}

/** Where to start a list, and how big its pages are (1 to 100). */
export interface PageOptions extends CallOptions {
  limit?: number;
  after?: string;
}

/** Claims of a buyer's session token (08 §11, D-34). */
export interface SessionClaims {
  /** Your identifier for the buyer, 1 to 128 characters. */
  sub: string;
  /** The event's key. */
  event: string;
  /** Seconds until it expires. Default 900. */
  ttlSeconds?: number;
  /** At most this many objects in a hold token created with it. */
  maxObjects?: number;
  /** Channel keys (no effect until channels exist). */
  channels?: string[];
}

/** Every item of every page from `fetch`, following `next` exactly: a page
 * that is empty is not the end, and a cursor that does not move is an error
 * rather than a loop. */
function paginate<T>(
  fetch: (after: string | undefined) => Promise<Page<T>>,
  start: string | undefined,
): Paginated<T> {
  async function* pages(): AsyncGenerator<Page<T>> {
    let after = start;
    for (;;) {
      const page = await fetch(after);
      yield page;
      if (page.next === null) {
        return;
      }
      if (page.next === after) {
        throw new TypeError("the server returned the same cursor twice");
      }
      after = page.next;
    }
  }
  return {
    pages,
    async *[Symbol.asyncIterator]() {
      for await (const page of pages()) {
        yield* page.data;
      }
    },
  };
}

const inBrowser = (): boolean => "window" in globalThis && "document" in globalThis;

const encoder = new TextEncoder();
const base64url = (bytes: Uint8Array): string =>
  btoa(String.fromCharCode(...bytes))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");

/** The client. Keys are kept out of every object it exposes. */
export class Cavea {
  readonly #send: Send;
  readonly #sign: (claims: SessionClaims) => Promise<string>;

  constructor(options: CaveaOptions) {
    if (inBrowser() && options.dangerouslyAllowBrowser !== true) {
      throw new Error(
        "@cavea/server-sdk holds a secret key and must not run in a browser: call your own backend from the page",
      );
    }
    let base: URL;
    try {
      base = new URL(options.baseUrl);
    } catch {
      throw new TypeError("baseUrl must be an absolute URL");
    }
    if (
      (base.protocol !== "https:" && base.protocol !== "http:") ||
      base.search !== "" ||
      base.hash !== ""
    ) {
      throw new TypeError("baseUrl must be an http or https URL without a query or fragment");
    }
    if (base.username !== "" || base.password !== "") {
      throw new TypeError("baseUrl must not carry credentials");
    }
    const { secretKey, adminKey, keyId } = options;
    if (secretKey === undefined && adminKey === undefined) {
      throw new TypeError("give secretKey, adminKey or both");
    }
    if (secretKey !== undefined && !/^sk_[0-9a-f]{64}$/.test(secretKey)) {
      throw new TypeError(
        secretKey.startsWith("pk_")
          ? "secretKey is a publishable key; give a secret key (sk_…)"
          : "secretKey is not a secret key (sk_ and 64 hex digits)",
      );
    }
    if (adminKey !== undefined && adminKey.length === 0) {
      throw new TypeError("adminKey is empty");
    }
    if (keyId !== undefined && !/^key_[0-9a-f]{32}$/.test(keyId)) {
      throw new TypeError("keyId is not a key id (key_ and 32 hex digits)");
    }
    const fetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.#send = transport({
      baseUrl: base.href,
      secretKey,
      adminKey,
      fetch,
      timeoutMs: options.timeoutMs ?? 30_000,
      maxRetries: options.maxRetries ?? 2,
      maxRetryAfter: options.maxRetryAfter ?? 30,
      backoffMs: 250,
    });
    this.#sign = async (claims) => {
      if (secretKey === undefined || keyId === undefined) {
        throw new TypeError("signing a session token needs secretKey and its keyId");
      }
      const { sub, event, ttlSeconds = 900, maxObjects, channels } = claims;
      if (typeof sub !== "string" || sub.length === 0 || [...sub].length > 128) {
        throw new TypeError("sub must be 1 to 128 characters");
      }
      if (typeof event !== "string" || event.length === 0) {
        throw new TypeError("event must be an event key");
      }
      if (!Number.isInteger(ttlSeconds) || ttlSeconds <= 0) {
        throw new TypeError("ttlSeconds must be a positive whole number");
      }
      if (maxObjects !== undefined && (!Number.isInteger(maxObjects) || maxObjects <= 0)) {
        throw new TypeError("maxObjects must be a positive whole number");
      }
      const payload: Record<string, unknown> = {
        sub,
        event,
        exp: Math.floor(Date.now() / 1000) + ttlSeconds,
      };
      if (maxObjects !== undefined) {
        payload["max_objects"] = maxObjects;
      }
      if (channels !== undefined) {
        payload["channels"] = channels;
      }
      const head = base64url(
        encoder.encode(JSON.stringify({ alg: "HS256", typ: "JWT", kid: keyId })),
      );
      const body = base64url(encoder.encode(JSON.stringify(payload)));
      // HMAC keyed with the key's text, as every JWT library does with an
      // HS256 secret; the server verifies with its SHA-256 (D-34).
      const hmac = await crypto.subtle.importKey(
        "raw",
        encoder.encode(secretKey),
        { name: "HMAC", hash: "SHA-256" },
        false,
        ["sign"],
      );
      const signature = await crypto.subtle.sign("HMAC", hmac, encoder.encode(`${head}.${body}`));
      return `${head}.${body}.${base64url(new Uint8Array(signature))}`;
    };
  }

  /** Any operation of the table, by its `operationId`: the reply, or a
   * `CaveaError`. */
  async call<Op extends OperationId>(
    op: Op,
    input: Input<Op>,
    options?: CallOptions,
  ): Promise<Reply<Op>> {
    return (await this.#send(op, input, options)).reply;
  }

  /** `call`, with the answer's status and headers. */
  callWithAnswer<Op extends OperationId>(
    op: Op,
    input: Input<Op>,
    options?: CallOptions,
  ): Promise<Answer<Reply<Op>>> {
    return this.#send(op, input, options);
  }

  /** A session token for one buyer on one event (08 §11), signed here with
   * the secret key: the page sends it with the buyer's holds. Needs `keyId`. */
  signSessionToken(claims: SessionClaims): Promise<string> {
    return this.#sign(claims);
  }

  #page<Op extends OperationId, T>(
    op: Op,
    input: (query: { limit?: number; after?: string }) => Input<Op>,
    options: PageOptions,
  ): Paginated<T> {
    const { limit, after, ...call } = options;
    return paginate(async (cursor) => {
      const query: { limit?: number; after?: string } = {};
      if (limit !== undefined) {
        query.limit = limit;
      }
      if (cursor !== undefined) {
        query.after = cursor;
      }
      return (await this.call(op, input(query), call)) as unknown as Page<T>;
    }, after);
  }

  readonly charts = {
    /** Every chart, archived ones included, oldest first. */
    list: (options: PageOptions = {}): Paginated<Chart> =>
      this.#page("listCharts", (query) => ({ query }), options),
    create: (body: CreateChart, options?: CallOptions): Promise<Chart> =>
      this.call("createChart", { body }, options),
    get: (chart: string, options?: CallOptions): Promise<Chart> =>
      this.call("getChart", { path: { chart } }, options),
    /** Archive: the key stays taken, versions keep serving. Idempotent. */
    archive: (chart: string, options?: CallOptions): Promise<Chart> =>
      this.call("archiveChart", { path: { chart } }, options),
    /** The draft, with its etag for `saveDraft`'s `ifMatch`. */
    getDraft: async (
      chart: string,
      options?: CallOptions,
    ): Promise<{ document: ChartDocument; etag: string }> => {
      const answer = await this.#send("getDraft", { path: { chart } }, options);
      return { document: answer.reply, etag: answer.headers.get("ETag") ?? "" };
    },
    /** Replace the draft. `ifMatch` is the etag it was read with, or `"*"` to
     * overwrite whatever is there; a stale one is `version_conflict`. */
    saveDraft: (
      chart: string,
      document: ChartDocument,
      options: CallOptions & { ifMatch: string },
    ): Promise<Chart> => {
      const { ifMatch, ...call } = options;
      return this.call(
        "saveDraft",
        { path: { chart }, headers: { "If-Match": ifMatch }, body: document },
        call,
      );
    },
    validate: (chart: string, options?: CallOptions): Promise<Validation> =>
      this.call("validateDraft", { path: { chart } }, options),
    /** Compile and publish the draft (only if its etag is still `ifMatch`,
     * when given). `created` is false when the draft compiled to a version
     * the chart already had. */
    publish: async (
      chart: string,
      options: CallOptions & { ifMatch?: string } = {},
    ): Promise<{ version: Version; created: boolean }> => {
      const { ifMatch, ...call } = options;
      const answer = await this.#send(
        "publishChart",
        { path: { chart }, ...(ifMatch === undefined ? {} : { headers: { "If-Match": ifMatch } }) },
        call,
      );
      return { version: answer.reply, created: answer.status === 201 };
    },
    /** A chart's versions, oldest first. */
    versions: (chart: string, options: PageOptions = {}): Paginated<Version> =>
      this.#page("listVersions", (query) => ({ path: { chart }, query }), options),
  };

  readonly events = {
    /** Every event, archived ones included, oldest first; `chart` keeps one
     * chart's. */
    list: (options: PageOptions & { chart?: string } = {}): Paginated<Event> => {
      const { chart, ...rest } = options;
      return this.#page(
        "listEvents",
        (query) => ({ query: chart === undefined ? query : { ...query, chart } }),
        rest,
      );
    },
    create: (body: CreateEvent, options?: CallOptions): Promise<Event> =>
      this.call("createEvent", { body }, options),
    /** The event with its counts. */
    get: (event: string, options?: CallOptions): Promise<Event> =>
      this.call("getEvent", { path: { event } }, options),
    update: (event: string, body: UpdateEvent, options?: CallOptions): Promise<Event> =>
      this.call("updateEvent", { path: { event }, body }, options),
    /** Archive: read-only from now on. Idempotent. */
    archive: (event: string, options?: CallOptions): Promise<Event> =>
      this.call("archiveEvent", { path: { event } }, options),
    /** Hold objects under a hold token. */
    hold: (event: string, body: Hold, options?: CallOptions): Promise<HoldReply> =>
      this.call("hold", { path: { event }, body }, options),
    /** Release a token's held objects (all of them without `objects`), or,
     * without a token, booked objects. */
    release: (event: string, body: Release, options?: CallOptions): Promise<SeatReply> =>
      this.call("release", { path: { event }, body }, options),
    /** Book a token's objects (all of them without `objects`), or free
     * objects directly. Pass your order number as `idempotencyKey`. */
    book: (event: string, body: Book, options?: CallOptions): Promise<SeatReply> =>
      this.call("book", { path: { event }, body }, options),
    /** Block, unblock or set a custom status. */
    changeStatus: (event: string, body: ChangeStatus, options?: CallOptions): Promise<SeatReply> =>
      this.call("changeStatus", { path: { event }, body }, options),
    /** The named objects (at most 1,000), and the labels the chart lacks. */
    objects: async (
      event: string,
      labels: string[],
      options?: CallOptions,
    ): Promise<{ data: EventObject[]; unknown: string[] }> =>
      (await this.call("listObjects", { path: { event }, query: { label: labels } }, options)) as {
        data: EventObject[];
        unknown: string[];
      },
    /** Every object, or every object of one status, in chart order. */
    listObjects: (
      event: string,
      options: PageOptions & { status?: "free" | "held" | "booked" | "blocked" | number } = {},
    ): Paginated<EventObject> => {
      const { status, ...rest } = options;
      return this.#page(
        "listObjects",
        (query) => ({
          path: { event },
          query: status === undefined ? query : { ...query, status: String(status) },
        }),
        rest,
      );
    },
  };

  readonly holdTokens = {
    /** A new hold token (`hold_token`, shown only here). Not retried: a
     * repeat would make a second token. */
    create: (body: CreateHoldToken, options?: CallOptions): Promise<HoldTokenCreated> =>
      this.call("createHoldToken", { body }, options),
    get: (token: string, options?: CallOptions): Promise<HoldTokenState> =>
      this.call("getHoldToken", { path: { token } }, options),
    /** A new expiry, from now. */
    extend: (
      token: string,
      body: ExtendHoldToken = {},
      options?: CallOptions,
    ): Promise<ExtendReply> => this.call("extendHoldToken", { path: { token }, body }, options),
    /** Release everything it holds; the token stays valid until it expires. */
    release: (token: string, options?: CallOptions): Promise<SeatReply> =>
      this.call("releaseHoldToken", { path: { token } }, options),
  };

  readonly keys = {
    /** The workspace's keys, revoked ones included (never the keys themselves). */
    list: (options: PageOptions = {}): Paginated<Key> =>
      this.#page("listKeys", (query) => ({ query }), options),
    /** A new key; `key` is shown only here. */
    create: (body: CreateKey, options?: CallOptions): Promise<KeyCreated> =>
      this.call("createKey", { body }, options),
    /** Revoke a key by its id. Idempotent; a key cannot revoke itself. */
    revoke: (id: string, options?: CallOptions): Promise<Key> =>
      this.call("revokeKey", { path: { id } }, options),
  };

  readonly assets = {
    /** Upload a PNG, JPEG or WebP image. `created` is false when the
     * workspace already had these bytes. */
    upload: async (
      bytes: Blob | ArrayBuffer | Uint8Array<ArrayBuffer>,
      options?: CallOptions,
    ): Promise<{ asset: Asset; created: boolean }> => {
      const body = bytes instanceof Blob ? bytes : new Blob([bytes]);
      const answer = await this.#send("uploadAsset", { body }, options);
      return { asset: answer.reply, created: answer.status === 201 };
    },
  };

  /** The operator's routes: the admin key. */
  readonly workspaces = {
    list: (options: PageOptions = {}): Paginated<Workspace> =>
      this.#page("listWorkspaces", (query) => ({ query }), options),
    /** A workspace and its first secret key, shown only here. */
    create: (body: CreateWorkspace, options?: CallOptions): Promise<WorkspaceCreated> =>
      this.call("createWorkspace", { body }, options),
    /** Another secret key for workspace `id` (`ws_…`). */
    mintKey: (id: string, body: MintKey, options?: CallOptions): Promise<KeyCreated> =>
      this.call("mintWorkspaceKey", { path: { workspace: id }, body }, options),
    /** An owner invitation to workspace `id`, with its one-time link. */
    inviteOwner: (
      id: string,
      body: InviteOwner,
      options?: CallOptions,
    ): Promise<InvitationCreated> =>
      this.call("inviteWorkspaceOwner", { path: { workspace: id }, body }, options),
  };
}
