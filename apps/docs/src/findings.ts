// Keeps the designer guide's list of validation findings
// (`content/designer/checking.md`) true to the product. The explanations are
// written by hand; the build fails unless
//
// - every code the compiler can report (`Code::as_str` in
//   `crates/cavea-compiler/src/finding.rs`) has a title in the console's
//   catalogue (`validation.<code>` in `apps/console/src/i18n/messages.ts`),
//   which is what a person sees in the Validation panel, and
// - the page has a level-3 heading with exactly that title, under `## Errors`
//   for an error and `## Warnings` for a warning, whose section names the code
//   (`Code E001.`), and no other level-3 heading there.
//
// So a new finding fails the build until someone writes what it means.

import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { catalogue } from "../../console/src/i18n/messages.ts";

/** The codes the compiler reports, in its order. */
export async function findingCodes(root: string): Promise<string[]> {
  const rust = await readFile(join(root, "crates/cavea-compiler/src/finding.rs"), "utf8");
  const codes = [...rust.matchAll(/Self::\w+ => "((?:[EW]\d{3})|unsupported)"/g)].map(
    (m) => m[1] ?? "",
  );
  return [...new Set(codes)];
}

/** What is wrong with `page` (the Markdown of checking.md) against `codes`. */
export function findings(codes: string[], page: string): string[] {
  const problems: string[] = [];
  if (codes.length < 20) {
    return [`found only ${codes.length} finding codes in finding.rs: the scan needs updating`];
  }
  const titles = catalogue as Record<string, string>;
  // Each level-3 heading, with the level-2 heading it is under and its text.
  const sections = new Map<string, { group: string; text: string }>();
  let group = "";
  let current: { group: string; text: string } | null = null;
  for (const line of page.split("\n")) {
    const h2 = line.match(/^##\s+(.+?)\s*$/);
    const h3 = line.match(/^###\s+(.+?)\s*$/);
    if (h2 && !h3) {
      group = h2[1] ?? "";
      current = null;
    } else if (h3) {
      current = { group, text: "" };
      if (group === "Errors" || group === "Warnings") sections.set(h3[1] ?? "", current);
    } else if (current !== null) {
      current.text += `${line}\n`;
    }
  }
  const expected = new Set<string>();
  for (const code of codes) {
    const title = titles[`validation.${code}`];
    if (title === undefined) {
      problems.push(
        `finding ${code} has no title in the console (validation.${code} in messages.ts)`,
      );
      continue;
    }
    expected.add(title);
    const section = sections.get(title);
    const under = code.startsWith("W") ? "Warnings" : "Errors";
    if (section === undefined) {
      problems.push(
        `designer/checking.md does not explain ${code}: add "### ${title}" under "## ${under}"`,
      );
    } else if (section.group !== under) {
      problems.push(`designer/checking.md: "${title}" (${code}) belongs under "## ${under}"`);
    } else if (
      !section.text.includes(code === "unsupported" ? `Code "${code}"` : `Code ${code}.`)
    ) {
      problems.push(`designer/checking.md: the section "${title}" does not name its code ${code}`);
    }
  }
  for (const title of sections.keys()) {
    if (!expected.has(title)) {
      problems.push(`designer/checking.md explains "${title}", which no finding code is called`);
    }
  }
  return problems;
}
