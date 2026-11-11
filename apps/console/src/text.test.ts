// Every user-supplied string is text in the console, never markup (12 §3
// T7, SEC-005). Solid's `{value}` in JSX inserts a text node, and an
// attribute's value is set as a value; the only ways to turn a string into
// markup or code are the sinks below. The lint (`no-restricted-properties`
// in `.oxlintrc.json`) refuses the property forms; it cannot see Solid's JSX
// props (`innerHTML={…}`), so this test reads every source file of the
// console for any of them. The surfaces it covers today: chart names and
// keys (`console/Charts.tsx`), workspace names and the user's email
// (`console/Frame.tsx`, `console/Account.tsx`), event names and keys
// (`console/Events.tsx`), API key names and origins
// (`console/Developers.tsx`), members' emails, invitations and the audit log
// (`console/Settings.tsx`, `console/Invite.tsx`), sign-in and setup, the
// server's problem details (`console/errors.ts`, `console/refusals.ts`,
// `ui/form.tsx`), and the designer's chart content (`designer/`). Order
// references (the event manager) are not shown anywhere yet; a new screen is
// read by this scan when it lands.
import { expect, test } from "bun:test";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const SRC = import.meta.dir;

const SINKS: Array<[RegExp, string]> = [
  [/\binnerHTML\b/, "innerHTML"],
  [/\bouterHTML\b/, "outerHTML"],
  [/\binsertAdjacentHTML\b/, "insertAdjacentHTML"],
  [/\b(set|parse)HTMLUnsafe\b/, "setHTMLUnsafe / parseHTMLUnsafe"],
  [/\bcreateContextualFragment\b/, "createContextualFragment"],
  [/\bDOMParser\b/, "DOMParser"],
  [/\bsrcdoc\b/, "srcdoc"],
  [/\bdocument\s*\.\s*write(ln)?\b/, "document.write"],
  [/javascript:/i, "a javascript: URL"],
  [/\bnew\s+Function\b|\beval\s*\(/, "eval or new Function"],
  [/<iframe\b/i, "an iframe"],
  // Solid's `<Dynamic component={…}>` makes an element named by a value.
  [/<Dynamic\b/, "Dynamic"],
];

async function sources(dir: string): Promise<string[]> {
  const found: string[] = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      found.push(...(await sources(path)));
    } else if (/\.tsx?$/.test(entry.name) && !entry.name.endsWith(".test.ts")) {
      found.push(path);
    }
  }
  return found;
}

test("no source of the console turns a string into markup or code (12 §3 T7)", async () => {
  const files = await sources(SRC);
  const names = files.map((f) => f.slice(SRC.length + 1));
  // The surfaces above are among what was read.
  for (const surface of [
    "console/Charts.tsx",
    "console/Frame.tsx",
    "console/Account.tsx",
    "console/Events.tsx",
    "console/Developers.tsx",
    "console/Settings.tsx",
    "console/Invite.tsx",
    "console/errors.ts",
    "ui/form.tsx",
  ]) {
    expect(names).toContain(surface);
  }
  expect(names.some((n) => n.startsWith("designer/"))).toBe(true);
  const found: string[] = [];
  for (const file of files) {
    const lines = (await readFile(file, "utf8")).split("\n");
    lines.forEach((line, i) => {
      for (const [sink, name] of SINKS) {
        if (sink.test(line)) {
          found.push(`${file.slice(SRC.length + 1)}:${i + 1}: ${name}`);
        }
      }
    });
  }
  expect(found).toEqual([]);
});
