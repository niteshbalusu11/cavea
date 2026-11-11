// For the tests: the designer's real WASM module (`cargo xtask
// build-designer`), loaded once per test file.
import { loadDesigner, type Designer } from "@cavea/designer-core";

const wasm = new URL(
  "../../../../packages/designer-core/wasm/cavea_designer_bg.wasm",
  import.meta.url,
);

export async function designer(): Promise<Designer> {
  const file = Bun.file(wasm);
  if (!(await file.exists())) {
    throw new Error("the designer WASM is missing: run `cargo xtask build-designer` first");
  }
  return loadDesigner({ wasm: await file.arrayBuffer() });
}
