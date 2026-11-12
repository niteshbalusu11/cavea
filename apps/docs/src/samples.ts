// Writes every code block marked `check=types` in `content/` to
// `samples/` (gitignored), with the site's variables filled in and its
// hidden `#hide ` lines shown, so that `tsc` (apps/docs' `typecheck`)
// compiles each against the workspace's own packages: `@cavea/renderer`,
// `@cavea/react` and `@cavea/server-sdk` as their declarations say. A `js`
// or `ts` block is compiled as TypeScript, a `tsx` or `jsx` block as TSX.
//
//   bun apps/docs/src/samples.ts   (then tsc --noEmit -p apps/docs/tsconfig.samples.json)

import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fences, substitute } from "./markdown.ts";

const HERE = join(import.meta.dir, "..");
const OUT = join(HERE, "samples");

async function pages(dir: string): Promise<string[]> {
  const out: string[] = [];
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...(await pages(p)));
    else if (e.name.endsWith(".md")) out.push(p);
  }
  return out.toSorted();
}

const site = JSON.parse(await readFile(join(HERE, "site.json"), "utf8")) as {
  vars: Record<string, string>;
};
await rm(OUT, { recursive: true, force: true });
await mkdir(OUT, { recursive: true });
let n = 0;
for (const page of await pages(join(HERE, "content"))) {
  const name = relative(join(HERE, "content"), page).replace(/\.md$/, "").replace(/\W+/g, "-");
  for (const f of fences(await readFile(page, "utf8"))) {
    if (f.meta.check !== "types") continue;
    const ext = f.lang === "tsx" || f.lang === "jsx" ? "tsx" : "ts";
    const code = substitute(f.code, site.vars, page).replace(/^(\s*)#hide ?/gm, "$1");
    const head = `// ${relative(HERE, page)}, line ${f.line}\n`;
    await writeFile(join(OUT, `${name}-${f.line}.${ext}`), `${head}${code}\nexport {};\n`);
    n++;
  }
}
console.log(`wrote ${n} samples to ${relative(process.cwd(), OUT) || "."}`);
