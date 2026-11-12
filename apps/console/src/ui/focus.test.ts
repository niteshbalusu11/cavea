// Where focus goes when the element that had it is gone (11 §8).
import { expect, test } from "bun:test";
import { replacement, type Page } from "./focus.ts";

const page = (connected: string[], ids: Record<string, string>, heading: string | null) =>
  ({
    connected: (el) => connected.includes(el),
    byFocusId: (id) => ids[id] ?? null,
    heading: () => heading,
  }) satisfies Page<string>;

test("focus returns to its opener while it is there, else to its stand-in, else the heading", () => {
  const opener = { el: "edit-button", focusId: "origins-key_1" };
  // Still on the page: nothing to do, the dialog returns focus there.
  expect(replacement(opener, page(["edit-button"], {}, "h1"))).toBeNull();
  // Its row was drawn again: the same control in the new row.
  expect(replacement(opener, page([], { "origins-key_1": "new-edit-button" }, "h1"))).toBe(
    "new-edit-button",
  );
  // Its row is gone (revoked and hidden, archived, deleted): the heading.
  expect(replacement(opener, page([], {}, "h1"))).toBe("h1");
  expect(replacement({ el: "x", focusId: null }, page([], { x: "y" }, "h1"))).toBe("h1");
  // Nothing had focus: the heading, not the body.
  expect(replacement(null, page([], {}, "main"))).toBe("main");
});
