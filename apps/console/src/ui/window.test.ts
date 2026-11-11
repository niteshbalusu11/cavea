import { expect, test } from "bun:test";
import { scrollToShow, windowFor } from "./window.ts";

test("the window covers the visible rows and the overscan, inside the list", () => {
  expect(windowFor(0, 280, 28, 1000, 5)).toEqual({ start: 0, count: 16 });
  expect(windowFor(2800, 280, 28, 1000, 5)).toEqual({ start: 95, count: 21 });
  // Near the end, and scrolled past it (a list that just shrank).
  expect(windowFor(27_720, 280, 28, 1000, 5)).toEqual({ start: 985, count: 15 });
  expect(windowFor(1e9, 280, 28, 10, 5)).toEqual({ start: 9, count: 1 });
  expect(windowFor(-50, 280, 28, 3, 5)).toEqual({ start: 0, count: 3 });
  expect(windowFor(0, 0, 28, 1000, 5)).toEqual({ start: 0, count: 0 });
  expect(windowFor(0, 280, 28, 0, 5)).toEqual({ start: 0, count: 0 });
});

test("scrolling to a row moves only as far as it must", () => {
  expect(scrollToShow(5, 0, 280, 28)).toBe(0);
  expect(scrollToShow(20, 0, 280, 28)).toBe(21 * 28 - 280);
  expect(scrollToShow(2, 280, 280, 28)).toBe(56);
});
