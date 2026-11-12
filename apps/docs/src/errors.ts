// Keeps the error reference (`content/reference/errors.md`) true to the
// server. The page is written by hand, since what an error means and what to
// do about it is prose, but the build fails unless, against the OpenAPI
// document's `Problem` schema,
//
// - its codes are exactly the `code` values, and
// - under each code, its reasons are exactly the ones the document lists for
//   that code: `x-cavea-reasons` (`details.reason`) and
//   `x-cavea-object-reasons` (each `details.objects[].reason`), and
// - each code in `x-cavea-reserved-codes` (listed, never sent) says
//   "Reserved" in its section.
//
// The server declares every reason in one place, the type its handlers take
// them from, and the document is generated from it, so the page is checked
// against what the server can send, in both directions.
//
// In the page, a code is a level-3 heading `### \`code\`` and a reason is a
// table row starting `| \`reason\` |` under it.

import { problemCodes, type Doc } from "./api.ts";

export interface Catalogue {
  codes: Set<string>;
  /** Code to the reasons documented under it. */
  reasons: Map<string, Set<string>>;
  /** Code to the text of its section. */
  text: Map<string, string>;
}

export function parseCatalogue(markdown: string): Catalogue {
  const codes = new Set<string>();
  const reasons = new Map<string, Set<string>>();
  const text = new Map<string, string>();
  let code: string | null = null;
  for (const line of markdown.split("\n")) {
    const heading = line.match(/^###\s+`([a-z_]+)`\s*$/);
    if (heading) {
      code = heading[1] ?? null;
      if (code !== null) {
        codes.add(code);
        reasons.set(code, reasons.get(code) ?? new Set());
      }
      continue;
    }
    if (/^#{1,2}\s/.test(line) || line.startsWith("<h2")) code = null;
    if (code === null) continue;
    text.set(code, `${text.get(code) ?? ""}${line}\n`);
    const row = line.match(/^\|\s*`([a-z_]+)`\s*\|/);
    if (row) reasons.get(code)?.add(row[1] ?? "");
  }
  return { codes, reasons, text };
}

/** What the document declares of errors. */
export interface Declared {
  codes: string[];
  /** Code to every reason it may carry, object reasons included. */
  reasons: Map<string, Set<string>>;
  reserved: string[];
}

export function declared(doc: Doc): Declared {
  const problem = (doc.components.schemas.Problem ?? {}) as Record<string, unknown>;
  const reasons = new Map<string, Set<string>>();
  for (const map of ["x-cavea-reasons", "x-cavea-object-reasons"]) {
    const byCode = (problem[map] ?? {}) as Record<string, string[]>;
    for (const [code, list] of Object.entries(byCode)) {
      const set = reasons.get(code) ?? new Set<string>();
      for (const reason of list) set.add(reason);
      reasons.set(code, set);
    }
  }
  const reserved = (problem["x-cavea-reserved-codes"] ?? []) as string[];
  return { codes: problemCodes(doc), reasons, reserved };
}

/** Problems with the catalogue, empty when it is true to the server. */
export function compare(catalogue: Catalogue, server: Declared): string[] {
  const problems: string[] = [];
  for (const code of server.codes) {
    if (!catalogue.codes.has(code)) problems.push(`code ${code} is not documented`);
  }
  for (const code of catalogue.codes) {
    if (!server.codes.includes(code))
      problems.push(`code ${code} is documented but the server has none`);
  }
  for (const code of new Set([...server.codes, ...catalogue.codes])) {
    const sent = server.reasons.get(code) ?? new Set<string>();
    const written = catalogue.reasons.get(code) ?? new Set<string>();
    for (const reason of sent) {
      if (!written.has(reason)) problems.push(`reason ${reason} of ${code} is not documented`);
    }
    for (const reason of written) {
      if (!sent.has(reason)) {
        problems.push(`reason ${reason} is documented under ${code}, which never sends it`);
      }
    }
  }
  for (const code of server.reserved) {
    if (!(catalogue.text.get(code) ?? "").includes("Reserved")) {
      problems.push(`code ${code} is reserved (never sent): its section must say "Reserved"`);
    }
  }
  return problems;
}
