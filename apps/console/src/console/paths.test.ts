// Where sign-in sends a person back to: this console only.
import { expect, test } from "bun:test";
import { safeNext, signInFor } from "./paths.ts";

test("sign-in returns only to a path of this console", () => {
  expect(safeNext("/charts?q=1")).toBe("/charts?q=1");
  expect(safeNext("/account")).toBe("/account");
  for (const bad of [
    undefined,
    "",
    "charts",
    "https://elsewhere.example",
    "//elsewhere.example",
    "/\\elsewhere.example",
    "/sign-in",
    "/sign-in?next=/x",
    "/setup",
    "/console/setup",
  ]) {
    expect(safeNext(bad)).toBe("/charts");
  }
});

test("the sign-in address carries the return path", () => {
  expect(signInFor("/events?x=1")).toBe("/sign-in?next=%2Fevents%3Fx%3D1");
  expect(signInFor("/charts")).toBe("/sign-in");
  expect(signInFor("//elsewhere.example")).toBe("/sign-in");
});
