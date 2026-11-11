// The distributable build of `@cavea/react`: `bun run build` writes `dist/`,
// the package a customer installs (`bun pm pack` in it): `index.js` (an ES
// module importing `react` and `@cavea/renderer`, neither bundled),
// `index.d.ts`, and a `package.json` with React as a peer dependency.
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = import.meta.dir;
const dist = join(root, "dist");

await rm(dist, { recursive: true, force: true });
await mkdir(dist, { recursive: true });
const result = await Bun.build({
  entrypoints: [join(root, "src/index.ts")],
  outdir: dist,
  format: "esm",
  target: "browser",
  external: ["react", "@cavea/renderer"],
});
if (!result.success) {
  throw new AggregateError(result.logs, "bun build failed");
}
const tsc = Bun.spawnSync(["tsc", "-p", "tsconfig.dist.json", "--outDir", dist], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
});
if (tsc.exitCode !== 0) {
  throw new Error("tsc failed to emit declarations");
}
const source = JSON.parse(await readFile(join(root, "package.json"), "utf8")) as {
  name: string;
  version: string;
  peerDependencies: Record<string, string>;
};
const renderer = JSON.parse(await readFile(join(root, "../renderer/package.json"), "utf8")) as {
  version: string;
};
const pkg = {
  name: source.name,
  version: source.version,
  // Nothing is published before the release pipeline (OPS-001).
  private: true,
  description: "React component for the Cavea seat map",
  license: "UNLICENSED",
  type: "module",
  sideEffects: false,
  main: "./index.js",
  types: "./index.d.ts",
  exports: {
    ".": { types: "./index.d.ts", default: "./index.js" },
    "./package.json": "./package.json",
  },
  dependencies: { "@cavea/renderer": renderer.version },
  peerDependencies: source.peerDependencies,
};
await writeFile(join(dist, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
console.log("dist: index.js, index.d.ts, package.json");
