// Answers are checked before they become state.
import { expect, test } from "bun:test";
import { ApiError } from "./client.ts";
import { checkChartList, checkMe } from "./check.ts";

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

const chart = {
  key: "k",
  name: "N",
  draft_etag: '"1"',
  draft_updated_at: "2026-11-11T00:00:00Z",
  published_version: null,
  archived_at: null,
  created_at: "2026-11-11T00:00:00Z",
};

test("a page of charts is checked before the list shows it", () => {
  expect(checkChartList({ data: [chart], next: null }).data).toHaveLength(1);
  expect(checkChartList({ data: [], next: "k" }).next).toBe("k");
  for (const bad of [
    { data: [null], next: null },
    // No cursor at all would page forever.
    { data: [] },
    { data: [], next: "" },
    { data: [{ ...chart, name: 3 }], next: null },
    { data: [{ ...chart, published_version: { number: 1 } }], next: null },
    "<html>",
  ]) {
    expect(() => checkChartList(bad)).toThrow(ApiError);
  }
});
