// Keeps the examples (`examples/` at the repository's root, DOC-009) on the
// names and version the site publishes: every `@cavea/…` dependency at
// `vars.version`, and the plain page's script tag at `vars.renderer_script`.
// The examples are copied out as they are, so they hold the literal values;
// a release that changes the site's variables fails the build until they
// follow.

import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

export async function examples(root: string, vars: Record<string, string>): Promise<string[]> {
  const problems: string[] = [];
  const dir = join(root, "examples");
  for (const name of await readdir(dir)) {
    const pkg = JSON.parse(await readFile(join(dir, name, "package.json"), "utf8")) as Record<
      string,
      Record<string, string> | undefined
    >;
    for (const deps of [pkg["dependencies"], pkg["devDependencies"]]) {
      for (const [dep, version] of Object.entries(deps ?? {})) {
        if (dep.startsWith("@cavea/") && version !== vars["version"]) {
          problems.push(
            `examples/${name}/package.json: ${dep} is ${version}, not ${vars["version"]}`,
          );
        }
      }
    }
  }
  const page = await readFile(join(dir, "plain-page/index.html"), "utf8");
  if (!page.includes(`<script src="${vars["renderer_script"]}"></script>`)) {
    problems.push(`examples/plain-page/index.html does not load ${vars["renderer_script"]}`);
  }
  return problems;
}
