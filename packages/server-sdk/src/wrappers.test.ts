// Every operation a secret key or the admin key may call is reachable: each
// method `WRAPPERS` names sends exactly its operation's method and path.
// (`WRAPPERS` is typed against the generated table, so an operation the
// server gains fails the type check until it has a line.)
import { expect, test } from "bun:test";
import { OPERATIONS, type OperationId } from "./generated.ts";
import { Cavea } from "./index.ts";
import { WRAPPERS } from "./wrappers.ts";

const KEY = ["sk", "1".repeat(64)].join("_");

/** Arguments for each method: placeholders the server would refuse, which
 * is all the routing needs. */
const ARGS: Record<string, unknown[]> = {
  "assets.upload": [new Uint8Array([1])],
  "charts.create": [{ key: "k", name: "n" }],
  "charts.saveDraft": ["p", {}, { ifMatch: "*" }],
  "charts.setThumbnail": ["p", 1, { asset: "0".repeat(32) }],
  "keys.updateOrigins": ["p", { allowed_origins: null }],
  "events.create": [{ key: "k", chart: "c" }],
  "events.update": ["p", {}],
  "events.book": ["p", { order_ref: "o" }],
  "events.changeStatus": ["p", { objects: ["a"], status: "blocked" }],
  "events.hold": ["p", { hold_token: "t", objects: ["a"] }],
  "events.release": ["p", { objects: ["a"] }],
  "holdTokens.create": [{ event: "e" }],
  "keys.create": [{ kind: "secret", name: "n" }],
  "workspaces.create": [{ key: "k", name: "n" }],
  "workspaces.mintKey": ["p", { name: "n" }],
  "workspaces.inviteOwner": ["p", { email: "a@b.example" }],
  "workspaces.ownerPasswordReset": ["p", { email: "a@b.example" }],
};

test("every wrapped operation sends its own method and path", async () => {
  const seen: Array<{ method: string; path: string }> = [];
  // Each answer is the operation's first documented success.
  let answer: (() => Response) | undefined;
  const cavea = new Cavea({
    baseUrl: "https://seats.example",
    secretKey: KEY,
    adminKey: "admin-".padEnd(32, "x"),
    fetch: async (input, init) => {
      seen.push({ method: init.method ?? "GET", path: new URL(input).pathname });
      return answer?.() ?? new Response(null, { status: 500 });
    },
  });
  const wrapped: Array<[OperationId, string]> = [];
  for (const op of Object.keys(WRAPPERS) as OperationId[]) {
    const name: string | null = WRAPPERS[op];
    if (name !== null) {
      wrapped.push([op, name]);
    }
  }
  expect(wrapped.length).toBeGreaterThan(25);
  for (const [op, name] of wrapped) {
    const [group = "", method = ""] = name.split(".");
    const fn = (cavea as unknown as Record<string, Record<string, (...a: unknown[]) => unknown>>)[
      group
    ]?.[method];
    expect(typeof fn).toBe("function");
    seen.length = 0;
    const [status = "200", media = null] =
      Object.entries(OPERATIONS[op].replies as Record<string, string | null>)[0] ?? [];
    answer = () =>
      media === null
        ? new Response(null, { status: Number(status) })
        : new Response(JSON.stringify({ data: [], next: null }), {
            status: Number(status),
            headers: { "Content-Type": media },
          });
    const out = fn?.(...(ARGS[name] ?? ["p"]));
    if (out !== null && typeof out === "object" && Symbol.asyncIterator in out) {
      for await (const _ of out as AsyncIterable<unknown>) {
        // one empty page
      }
    } else {
      await out;
    }
    const info: { method: string; path: string } = OPERATIONS[op];
    const pattern = new RegExp(`^${info.path.replace(/\{\w+\}/g, "[^/]+")}$`);
    const last = seen.at(-1);
    expect(seen).toHaveLength(1);
    expect({ op, method: last?.method, path: pattern.test(last?.path ?? "") }).toEqual({
      op,
      method: info.method,
      path: true,
    });
  }
});
