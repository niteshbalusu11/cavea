import { expect, test } from "bun:test";
import { loadRenderer } from "./index.ts";

test("loads the WASM and returns the Rust crate version", async () => {
  const wasm = await Bun.file(new URL("../wasm/cavea_web_bg.wasm", import.meta.url)).arrayBuffer();
  const renderer = await loadRenderer(wasm);
  expect(renderer.version()).toMatch(/^\d+\.\d+\.\d+/);
});
