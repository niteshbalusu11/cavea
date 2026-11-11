import { describe, expect, test } from "bun:test";
import { match, rank } from "./match.ts";

const labels = [
  "Zoom to fit",
  "Zoom to selection",
  "Insert row block…",
  "Row block",
  "Arc block",
  "Select all",
  "Toggle snapping",
  "Distribute horizontally",
  "Undo",
];
const order = (q: string) => rank(labels, q, (l) => l).map((r) => r.item);

describe("the palette's matching", () => {
  test("letters in order, case and accents ignored; others do not match", () => {
    expect(match("zf", "Zoom to fit")?.at).toEqual([0, 8]);
    expect(match("ZOOM", "Zoom to fit")).not.toBeNull();
    expect(match("creme", "Crème")).not.toBeNull();
    expect(match("fz", "Zoom to fit")).toBeNull();
    expect(match("", "Anything")?.score).toBe(0);
  });

  test("a word start beats a match inside a word, which beats scattered letters", () => {
    expect(order("block")).toEqual(["Row block", "Arc block", "Insert row block…"]);
    expect(order("row")[0]).toBe("Row block");
    expect(order("sel")[0]).toBe("Select all");
    expect(order("sel")).toContain("Zoom to selection");
    expect(order("zts")).toEqual(["Zoom to selection"]);
  });

  test("ties keep the list's order, and an empty query lists everything as given", () => {
    expect(order("")).toEqual(labels);
    expect(order("zoom")).toEqual(["Zoom to fit", "Zoom to selection"]);
  });
});
