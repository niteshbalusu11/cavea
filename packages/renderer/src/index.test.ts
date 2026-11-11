import { expect, test } from "bun:test";
import { loadRenderer } from "./index.ts";

test("loads the WASM, retrying after a failed load", async () => {
  await expect(loadRenderer(new Uint8Array([0, 1, 2, 3]))).rejects.toThrow();

  const wasm = await Bun.file(new URL("../wasm/cavea_web_bg.wasm", import.meta.url)).arrayBuffer();
  const renderer = await loadRenderer(wasm);
  expect(renderer.version()).toMatch(/^\d+\.\d+\.\d+/);
});
