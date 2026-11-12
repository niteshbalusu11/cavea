// The tables' rules: a column sorts through the server, in the page once
// the list is whole, or not at all; sorting is stable; arrows stop at the
// ends.
import { expect, test } from "bun:test";
import { by, nextRow, sortBy, sorted, toggle } from "./tables.ts";

test("a column the server orders by always sorts; the others only once every row is loaded", () => {
  const events = { server: ["created"] as const, page: [] as const, complete: false };
  expect(sortBy("created", events)).toBe("server");
  expect(sortBy("name", { ...events, server: [] })).toBe("none");
  const keys = { server: [] as const, page: ["name", "created"] as const };
  // A partly loaded list is not sortable: the first loaded row is not the first of all.
  expect(sortBy("name", { ...keys, complete: false })).toBe("none");
  expect(sortBy("name", { ...keys, complete: true })).toBe("page");
  expect(sortBy("prefix" as "name", { ...keys, complete: true })).toBe("none");
});

test("a header toggles its own direction and starts another column afresh", () => {
  const first = toggle(null, "created", "descending");
  expect(first).toEqual({ column: "created", direction: "descending" });
  expect(toggle(first, "created")).toEqual({ column: "created", direction: "ascending" });
  expect(toggle(first, "name")).toEqual({ column: "name", direction: "ascending" });
});

test("sorting is stable, never changes the rows, and leaves the server's order when unsorted", () => {
  const rows = [
    { name: "b", used: null, n: 2 },
    { name: "A", used: "2026-11-02T00:00:00Z", n: 1 },
    { name: "a", used: "2026-11-01T00:00:00Z", n: 3 },
  ];
  const compare = {
    name: by.text((r: (typeof rows)[number]) => r.name),
    used: by.time((r: (typeof rows)[number]) => r.used),
  };
  expect(sorted(rows, null, compare)).toEqual(rows);
  // "A" and "a" compare equal: they keep the server's order.
  expect(sorted(rows, { column: "name", direction: "ascending" }, compare).map((r) => r.n)).toEqual(
    [1, 3, 2],
  );
  expect(
    sorted(rows, { column: "name", direction: "descending" }, compare).map((r) => r.n),
  ).toEqual([2, 1, 3]);
  // Never used sorts before any use.
  expect(sorted(rows, { column: "used", direction: "ascending" }, compare).map((r) => r.n)).toEqual(
    [2, 3, 1],
  );
  expect(rows.map((r) => r.n)).toEqual([2, 1, 3]);
});

test("row keys move one row, jump to the ends, and stop there", () => {
  expect(nextRow("ArrowDown", 0, 3)).toBe(1);
  expect(nextRow("ArrowDown", 2, 3)).toBe(2);
  expect(nextRow("ArrowUp", 0, 3)).toBe(0);
  expect(nextRow("Home", 2, 3)).toBe(0);
  expect(nextRow("End", 0, 3)).toBe(2);
  expect(nextRow("Enter", 0, 3)).toBeNull();
  expect(nextRow("ArrowDown", 0, 0)).toBeNull();
});
