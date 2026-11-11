// Paging as the API pages (08 §6.3): the cursor is passed back as `after`
// exactly, reading stops at `next: null` or at the bound, and a bounded
// read says whether it saw the whole list.
import { expect, test } from "bun:test";
import { loadAll, loadPages, type Page } from "./pages.ts";

/** A list of `n` rows served `size` at a time, recording each `after`. */
function server(n: number, size: number) {
  const rows = Array.from({ length: n }, (_, i) => `row${i}`);
  const asked: (string | undefined)[] = [];
  const page = async (after: string | undefined): Promise<Page<string>> => {
    asked.push(after);
    const start = after === undefined ? 0 : rows.indexOf(after) + 1;
    const data = rows.slice(start, start + size);
    const last = data.at(-1);
    return { data, next: start + size < n && last !== undefined ? last : null };
  };
  return { rows, asked, page };
}

test("pages follow the cursor and stop at the last page", async () => {
  const s = server(7, 3);
  const all = await loadPages(s.page, 10);
  expect(all).toEqual({ data: s.rows, next: null });
  expect(s.asked).toEqual([undefined, "row2", "row5"]);
});

test("a bounded read stops at its bound and keeps the cursor to go on", async () => {
  const s = server(7, 3);
  const two = await loadPages(s.page, 2);
  expect(two.data).toEqual(s.rows.slice(0, 6));
  expect(two.next).toBe("row5");
  // Going on from the cursor reads the rest, as "load more" does.
  const rest = await s.page(two.next ?? undefined);
  expect([...two.data, ...rest.data]).toEqual(s.rows);
});

test("a whole-list read says when the list goes on past its bound", async () => {
  expect(await loadAll(server(6, 3).page, 2)).toEqual({
    data: server(6, 3).rows,
    complete: true,
  });
  const partial = await loadAll(server(7, 3).page, 2);
  expect(partial.complete).toBe(false);
  expect(partial.data).toHaveLength(6);
  expect((await loadAll(server(0, 3).page, 2)).complete).toBe(true);
});
