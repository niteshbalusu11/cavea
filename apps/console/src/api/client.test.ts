// The request layer: what each request carries, how failures are read, the
// ended session, cancellation (09 §10).
import { beforeEach, expect, test } from "bun:test";
import { ApiError, call, callWithHeaders, configure, isAbort } from "./client.ts";

interface Seen {
  url: string;
  init: RequestInit;
}
let seen: Seen[] = [];
let answer: (url: string, init: RequestInit) => Promise<Response>;
let lost = 0;

const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });
const problem = (
  status: number,
  code: string,
  details: Record<string, unknown> = {},
  headers: Record<string, string> = {},
) =>
  new Response(
    JSON.stringify({ code, status, message: "From the server.", details, request_id: "req_body" }),
    {
      status,
      headers: {
        "Content-Type": "application/problem+json",
        "Cavea-Request-Id": "req_header",
        ...headers,
      },
    },
  );

beforeEach(() => {
  seen = [];
  lost = 0;
  answer = async () => json(200, {});
  configure({
    csrf: "csrf-placeholder",
    workspace: () => "ws_placeholder",
    onSessionLost: () => lost++,
    timeoutMs: 30_000,
    fetch: (url, init) => {
      seen.push({ url, init });
      return answer(url, init);
    },
  });
});

const header = (i: number, name: string) => new Headers(seen[i]?.init.headers).get(name);
const signal = () => new AbortController().signal;

test("an array query parameter is repeated, each value encoded", async () => {
  answer = async () => json(200, { data: [], unknown: [] });
  await call("listObjects", { path: { event: "e" }, query: { label: ["A-1", "B 2&x"] } }, signal());
  expect(seen[0]?.url).toBe("/v1/events/e/objects?label=A-1&label=B+2%26x");
});

test("a read sends the workspace and no CSRF token; a write sends both", async () => {
  await call("listCharts", { query: { limit: 100, after: "a b" } }, signal());
  answer = async () => json(201, { key: "k" });
  await call("createChart", { body: { key: "k", name: "N" } }, signal());
  await call("archiveChart", { path: { chart: "a/b c" } }, signal());

  expect(seen[0]?.url).toBe("/v1/charts?limit=100&after=a+b");
  expect(seen[0]?.init.method).toBe("GET");
  expect(seen[0]?.init.credentials).toBe("same-origin");
  expect(header(0, "Cavea-Workspace")).toBe("ws_placeholder");
  expect(header(0, "Cavea-Csrf-Token")).toBeNull();

  expect(seen[1]?.init.method).toBe("POST");
  expect(header(1, "Cavea-Workspace")).toBe("ws_placeholder");
  expect(header(1, "Cavea-Csrf-Token")).toBe("csrf-placeholder");
  expect(header(1, "Content-Type")).toBe("application/json");
  expect(seen[1]?.init.body).toBe(JSON.stringify({ key: "k", name: "N" }));

  // Path parameters are encoded: a key cannot reach another route.
  expect(seen[2]?.url).toBe("/v1/charts/a%2Fb%20c");
  expect(seen[2]?.init.method).toBe("DELETE");
  expect(header(2, "Cavea-Csrf-Token")).toBe("csrf-placeholder");
});

test("the session routes send no workspace; members routes do", async () => {
  await call("consoleSignIn", { body: { email: "e", password: "p" } }, signal());
  answer = async () => new Response(null, { status: 204 });
  await call("consoleSignOut", {}, signal());
  answer = async () => json(200, { data: [], next: null });
  await call("listMembers", {}, signal());

  expect(seen[0]?.url).toBe("/console/api/session");
  expect(header(0, "Cavea-Workspace")).toBeNull();
  expect(header(1, "Cavea-Workspace")).toBeNull();
  expect(header(1, "Cavea-Csrf-Token")).toBe("csrf-placeholder");
  expect(header(2, "Cavea-Workspace")).toBe("ws_placeholder");
});

test("no CSRF token is sent before there is one", async () => {
  configure({ csrf: null });
  await call("consoleSignIn", { body: { email: "e", password: "p" } }, signal());
  expect(header(0, "Cavea-Csrf-Token")).toBeNull();
});

test("a 204 resolves to undefined", async () => {
  answer = async () => new Response(null, { status: 204 });
  expect(await call("consoleSignOut", {}, signal())).toBeUndefined();
});

test("a problem becomes an ApiError with its code, reason, request id and Retry-After", async () => {
  answer = async () => problem(429, "rate_limited", { reason: "backoff" }, { "Retry-After": "7" });
  const error = await call("consoleSignIn", { body: { email: "e", password: "p" } }, signal()).then(
    () => null,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(ApiError);
  const e = error as ApiError;
  expect([e.status, e.code, e.reason, e.requestId, e.retryAfter]).toEqual([
    429,
    "rate_limited",
    "backoff",
    "req_header",
    7,
  ]);
  expect(e.message).toBe("From the server.");
});

test("an answer that is not a problem is `unexpected`, never a crash", async () => {
  const cases: Response[] = [
    new Response("<html>Bad gateway</html>", {
      status: 502,
      headers: { "Content-Type": "text/html" },
    }),
    new Response("{not json", {
      status: 500,
      headers: { "Content-Type": "application/problem+json" },
    }),
    json(400, { code: "not_a_code" }),
    new Response("{not json", { status: 200, headers: { "Content-Type": "application/json" } }),
  ];
  for (const response of cases) {
    answer = async () => response;
    const e = (await call("consoleMe", {}, signal()).catch((x: unknown) => x)) as ApiError;
    expect(e).toBeInstanceOf(ApiError);
    expect(e.code).toBe("unexpected");
    expect(e.status).toBe(response.status);
  }
});

test("no answer at all is `network`", async () => {
  answer = async () => {
    throw new TypeError("Failed to fetch");
  };
  const e = (await call("consoleMe", {}, signal()).catch((x: unknown) => x)) as ApiError;
  expect([e.code, e.status, e.requestId]).toEqual(["network", 0, null]);
});

test("a 401 that ends the session is reported once, and the call still rejects", async () => {
  answer = async () => problem(401, "unauthorized", { reason: "console_session" });
  const e = (await call("listCharts", {}, signal()).catch((x: unknown) => x)) as ApiError;
  expect(e.sessionLost).toBe(true);
  expect(lost).toBe(1);

  // No cookie at all reads the same.
  answer = async () => problem(401, "unauthorized", { reason: "missing" });
  await call("consoleMe", {}, signal()).catch(() => {});
  expect(lost).toBe(2);
});

test("a wrong password or setup token is not an ended session", async () => {
  for (const reason of ["credentials", "setup_token"]) {
    answer = async () => problem(401, "unauthorized", { reason });
    const e = (await call("consoleSignIn", { body: { email: "e", password: "p" } }, signal()).catch(
      (x: unknown) => x,
    )) as ApiError;
    expect(e.sessionLost).toBe(false);
  }
  expect(lost).toBe(0);
});

test("a cancelled request rejects with the abort, not an ApiError", async () => {
  const controller = new AbortController();
  answer = (_url, init) =>
    new Promise((_resolve, reject) => {
      init.signal?.addEventListener("abort", () => reject(init.signal?.reason));
    });
  const pending = call("listCharts", {}, controller.signal).catch((x: unknown) => x);
  controller.abort();
  const e = await pending;
  expect(e).not.toBeInstanceOf(ApiError);
  expect(isAbort(e)).toBe(true);
});

test("a request that takes too long fails with `timeout`", async () => {
  configure({ timeoutMs: 10 });
  answer = (_url, init) =>
    new Promise((_resolve, reject) => {
      init.signal?.addEventListener("abort", () => reject(init.signal?.reason));
    });
  const e = (await call("listCharts", {}, signal()).catch((x: unknown) => x)) as ApiError;
  expect(e).toBeInstanceOf(ApiError);
  expect(e.code).toBe("timeout");
});

test("a draft: its etag read from the answer, sent back in If-Match, in the pinned workspace", async () => {
  answer = async () => json(200, { schema: 1 }, { ETag: '"7"' });
  const got = await callWithHeaders("getDraft", { path: { chart: "hall" } }, signal(), {
    workspace: "ws_pinned",
  });
  expect(got.headers.get("ETag")).toBe('"7"');
  expect(header(0, "Cavea-Workspace")).toBe("ws_pinned");
  const draft = new Blob(['{"schema":1}'], { type: "application/json" });
  await call(
    "saveDraft",
    // A draft already serialised goes as it is.
    { path: { chart: "hall" }, headers: { "If-Match": '"7"' }, body: draft as never },
    signal(),
  );
  expect(header(1, "If-Match")).toBe('"7"');
  expect(header(1, "Cavea-Workspace")).toBe("ws_placeholder");
  expect(seen[1]?.init.body).toBe(draft);
});
