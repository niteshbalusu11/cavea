// Keeps the error reference (`content/reference/errors.md`) true to the
// server. The page is written by hand, since what an error means and what to
// do about it is prose, but the build fails unless
//
// - its codes are exactly the `code` values of the OpenAPI document's
//   `Problem` schema, and
// - its reasons are exactly the `reason` values the server's source can put
//   in `details` (found by the scan below), and
// - the scan understood every place the source sets a reason: a reason set
//   from anything but a string literal must be one of the indirections
//   listed in KNOWN, so a new kind of indirection fails the build until the
//   scan learns it.
//
// In the page, a code is a level-3 heading `### \`code\`` and a reason is a
// table row starting `| \`reason\` |` under it.

import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";

export interface Catalogue {
  codes: Set<string>;
  /** Reason to the codes it is documented under. */
  reasons: Map<string, Set<string>>;
}

export function parseCatalogue(markdown: string): Catalogue {
  const codes = new Set<string>();
  const reasons = new Map<string, Set<string>>();
  let code: string | null = null;
  for (const line of markdown.split("\n")) {
    const heading = line.match(/^###\s+`([a-z_]+)`\s*$/);
    if (heading) {
      code = heading[1] ?? null;
      if (code !== null) codes.add(code);
      continue;
    }
    if (/^#{1,2}\s/.test(line)) code = null;
    const row = line.match(/^\|\s*`([a-z_]+)`\s*\|/);
    if (row && code !== null) {
      const reason = row[1] ?? "";
      if (!reasons.has(reason)) reasons.set(reason, new Set());
      reasons.get(reason)?.add(code);
    }
  }
  return { codes, reasons };
}

async function rustFiles(dir: string): Promise<string[]> {
  const out: string[] = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "tests") out.push(...(await rustFiles(path)));
    } else if (entry.name.endsWith(".rs") && entry.name !== "tests.rs") {
      out.push(path);
    }
  }
  return out;
}

/** A source file without its `#[cfg(test)]` module. */
function withoutTests(text: string): string {
  const at = text.search(/#\[cfg\(test\)\]\s*mod tests/);
  return at < 0 ? text : text.slice(0, at);
}

/** The string arms of `impl NAME { … }`'s match: `Self::X => "x"`. */
function enumArms(text: string, name: string): string[] {
  const start = text.indexOf(`impl ${name} {`);
  if (start < 0) return [];
  const end = text.indexOf("\n}\n", start);
  const block = text.slice(start, end < 0 ? undefined : end);
  return [...block.matchAll(/Self::\w+(?:\s*\|\s*Self::\w+)*\s*=>\s*"([a-z_]+)"/g)].map(
    (m) => m[1] ?? "",
  );
}

/**
 * Places that set a reason from something other than a literal, and where
 * the scan finds the literals they pass on: `file: expression`.
 */
const KNOWN = new Set([
  // `unauthorized("missing")` and the like: the calls are scanned.
  "http/auth.rs: reason",
  // `invalid("json_depth", …)` and the like: the calls are scanned.
  "http/extract.rs: reason",
  // An engine `Failure::InvalidRequest`: `InvalidReason`'s arms are scanned.
  "http/error.rs: reason.code()",
  // An asset refused: `Refused`'s arms are scanned.
  "http/assets.rs: refused.reason()",
]);

export interface Scan {
  reasons: Set<string>;
  unknown: string[];
}

/** Every `details.reason` the server can send, from its source. */
export async function scanReasons(root: string): Promise<Scan> {
  const server = join(root, "crates/cavea-server/src");
  const files = [...(await rustFiles(server)), join(root, "crates/cavea-engine/src/types.rs")];
  const reasons = new Set<string>();
  const unknown: string[] = [];
  for (const file of files) {
    const text = withoutTests(await readFile(file, "utf8"));
    const short = relative(server, file);
    const literal = [
      /with_detail\(\s*"reason",\s*"([a-z_]+)"/g,
      /\b(?:invalid|unauthorized)\(\s*"([a-z_]+)"/g,
      /JsonRejection::\w+\(_\)\s*=>\s*"([a-z_]+)"/g,
      /_\s*=>\s*"(body_unreadable)"/g,
    ];
    for (const pattern of literal) {
      for (const m of text.matchAll(pattern)) reasons.add(m[1] ?? "");
    }
    for (const name of ["InvalidReason", "ObjectReason", "Refused"]) {
      for (const arm of enumArms(text, name)) reasons.add(arm);
    }
    for (const m of text.matchAll(/with_detail\(\s*"reason",\s*([^"\s][^)]*\)?)\s*\)/g)) {
      const where = `${short}: ${(m[1] ?? "").trim()}`;
      if (!KNOWN.has(where)) unknown.push(where);
    }
    for (const m of text.matchAll(/\b(?:invalid|unauthorized)\(\s*([^"\s)][^,)]*\)?)\s*[,)]/g)) {
      const expr = (m[1] ?? "").trim();
      // The helpers' own definitions take `reason: &'static str`.
      if (expr.startsWith("reason:")) continue;
      const where = `${short}: ${expr}`;
      if (!KNOWN.has(where)) unknown.push(where);
    }
  }
  return { reasons, unknown };
}

/** Problems with the catalogue, empty when it is true to the server. */
export function compare(catalogue: Catalogue, codes: string[], scan: Scan): string[] {
  const problems: string[] = [];
  for (const code of codes) {
    if (!catalogue.codes.has(code)) problems.push(`code ${code} is not documented`);
  }
  for (const code of catalogue.codes) {
    if (!codes.includes(code)) problems.push(`code ${code} is documented but the server has none`);
  }
  for (const reason of scan.reasons) {
    if (!catalogue.reasons.has(reason)) problems.push(`reason ${reason} is not documented`);
  }
  for (const reason of catalogue.reasons.keys()) {
    if (!scan.reasons.has(reason)) {
      problems.push(`reason ${reason} is documented but the server never sends it`);
    }
  }
  for (const where of scan.unknown) {
    problems.push(
      `a reason set from an expression the scan does not follow (${where}): teach apps/docs/src/errors.ts`,
    );
  }
  return problems;
}
