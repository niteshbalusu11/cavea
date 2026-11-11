// A `me` answer is checked before it becomes the session.
import { expect, test } from "bun:test";
import { ApiError } from "./client.ts";
import { checkMe } from "./me.ts";

const good = {
  user: { id: "usr_1", email: "a@example.org" },
  workspaces: [
    { id: "ws_1", key: "main", name: "Main", role: "owner", permissions: ["charts.read"] },
  ],
  csrf_token: "placeholder",
  session: { expires_at: "2026-11-11T00:00:00Z", absolute_expires_at: "2026-11-18T00:00:00Z" },
};

test("a well-formed answer is taken; anything else is `unexpected`", () => {
  expect(checkMe(good)).toBe(good as never);
  expect(checkMe({ ...good, workspaces: [] }).workspaces).toEqual([]);
  for (const bad of [
    {},
    "<html>",
    null,
    { ...good, workspaces: {} },
    { ...good, workspaces: [{ id: "ws_1" }] },
    { ...good, csrf_token: "" },
    { ...good, user: { id: "usr_1" } },
  ]) {
    expect(() => checkMe(bad)).toThrow(ApiError);
  }
});
