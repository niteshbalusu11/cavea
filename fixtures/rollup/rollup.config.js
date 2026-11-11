// Rollup resolves packages only with node-resolve, which any project has, and
// copies no file a module names with `new URL("./x", import.meta.url)`. The
// renderer's bundle names its WASM and atlas that way, relative to itself, so
// they must be copied beside the output: the one line a project adds is the
// plugin below (or a copy step in its build, or `assets.baseUrl` pointing at
// wherever they are served).
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { nodeResolve } from "@rollup/plugin-node-resolve";

const file = (name) => createRequire(import.meta.url).resolve(`@cavea/renderer/${name}`);
const caveaAssets = {
  name: "cavea-assets",
  generateBundle() {
    for (const name of ["cavea_web_bg.wasm", "glyphs.png"]) {
      this.emitFile({ type: "asset", fileName: name, source: readFileSync(file(name)) });
    }
  },
};

export default {
  input: "src/main.js",
  output: { dir: "dist", format: "es" },
  plugins: [nodeResolve(), caveaAssets],
};
