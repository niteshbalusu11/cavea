// Answers are checked before they become state.
import { expect, test } from "bun:test";
import { ApiError } from "./client.ts";
import { checkChartList, checkMe } from "./check.ts";

const good = {
  user: { id: "usr_1", email: "a@example.org", second_factor: false },
  workspaces: [
    { id: "ws_1", key: "main", name: "Main", role: "owner", permissions: ["charts.read"] },
  ],
  csrf_token: "placeholder",
  session: { expires_at: "2026-11-11T00:00:00Z", absolute_expires_at: "2026-11-18T00:00:00Z" },
};

test("a well-formed answer is taken; anything else is `unexpected`", () => {
  const expired = {
    state: "expired",
    read_only: true,
    grace_until: null,
    manage: false,
    source: null,
    reason: null,
    licence: null,
  };
  expect(checkMe({ ...good, licence: expired })).toEqual({ ...good, licence: expired } as never);
  // A licence the console cannot read shows nothing and closes nothing: the server decides.
  for (const licence of [undefined, null, { state: "expired" }, { ...expired, state: "soon" }]) {
    expect(checkMe({ ...good, licence }).licence).toMatchObject({
      state: "valid",
      read_only: false,
    });
  }
  expect(checkMe({ ...good, workspaces: [] }).workspaces).toEqual([]);
  for (const bad of [
    {},
    "<html>",
    null,
    { ...good, workspaces: {} },
    { ...good, workspaces: [{ id: "ws_1" }] },
    { ...good, csrf_token: "" },
    { ...good, user: { id: "usr_1" } },
    { ...good, user: { id: "usr_1", email: "a@example.org" } },
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
  events: { active: 2, archived: 1 },
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
    // A card's events count is drawn as a number: an older server has none.
    { data: [{ ...chart, events: undefined }], next: null },
    { data: [{ ...chart, events: { active: "2", archived: 1 } }], next: null },
    "<html>",
  ]) {
    expect(() => checkChartList(bad)).toThrow(ApiError);
  }
});
