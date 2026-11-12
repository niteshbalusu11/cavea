// Optimistic changes: shown at once, replaced by the server's answer, put
// back on a refusal, and two changes in flight settle on the server's truth.
import { expect, test } from "bun:test";
import { Optimistic } from "./optimistic.ts";

function role(start: string) {
  const shown: string[] = [];
  let raced = 0;
  const o = new Optimistic(
    start,
    (v) => shown.push(v),
    () => raced++,
  );
  return { o, shown, raced: () => raced };
}

test("a change shows at once and stays once the server agrees", () => {
  const { o, shown, raced } = role("viewer");
  const id = o.begin("editor");
  expect(o.shown).toBe("editor");
  o.confirm(id, "editor");
  expect(shown).toEqual(["editor", "editor"]);
  expect(o.busy).toBe(false);
  expect(raced()).toBe(0);
});

test("a refusal puts back what the server last said", () => {
  const { o, shown } = role("viewer");
  const id = o.begin("editor");
  expect(o.fail(id)).toBe("viewer");
  expect(shown.at(-1)).toBe("viewer");
  // A late or repeated answer to a settled change changes nothing.
  o.confirm(id, "editor");
  expect(o.shown).toBe("viewer");
});

test("two changes racing: the newer shows; answers in either order settle on the newer's, then the list is read again", () => {
  const { o, raced } = role("viewer");
  const a = o.begin("editor");
  const b = o.begin("admin");
  expect(o.shown).toBe("admin");
  // The newer answers first; the older's later answer does not override it.
  o.confirm(b, "admin");
  expect(o.shown).toBe("admin");
  o.confirm(a, "editor");
  expect(o.shown).toBe("admin");
  expect(raced()).toBe(1);
});

test("the newer of two refused while the older is on its way: the older shows, then the server's answer", () => {
  const { o, raced } = role("viewer");
  const a = o.begin("editor");
  const b = o.begin("admin");
  expect(o.fail(b)).toBe("editor");
  o.confirm(a, "editor");
  expect(o.shown).toBe("editor");
  expect(raced()).toBe(1);
});

test("both refused: back to the start", () => {
  const { o } = role("viewer");
  const a = o.begin("editor");
  const b = o.begin("admin");
  o.fail(a);
  expect(o.fail(b)).toBe("viewer");
});
