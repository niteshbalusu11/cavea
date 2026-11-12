// The public declarations of a package, read from the one file that holds
// them all (`packages/renderer/api.md`, which the renderer's build writes and
// refuses to leave stale) or from a TypeScript source. Only what the
// reference pages print is understood: interfaces and their members with doc
// comments, string unions with a doc comment per member, and the text of a
// declaration. Not a TypeScript parser: anything else is ignored, and a name
// asked for that is not found fails the build.

export interface Member {
  name: string;
  optional: boolean;
  /** The member's type, or a method's signature from its `(`; whitespace collapsed. */
  type: string;
  method: boolean;
  /** The type as written, doc comments inside it kept (empty for a method). */
  raw: string;
  /** The doc comment's text, or "". */
  doc: string;
}

const OPEN = new Set(["{", "(", "["]);
const CLOSE = new Set(["}", ")", "]"]);

/** The index just past the bracket that closes the one at `at`. */
function closing(text: string, at: number): number {
  let depth = 0;
  for (let i = at; i < text.length; i++) {
    const c = text[i] ?? "";
    if (text.startsWith("/*", i)) i = text.indexOf("*/", i) + 1;
    else if (c === '"' || c === "'" || c === "`") i = text.indexOf(c, i + 1);
    else if (OPEN.has(c)) depth++;
    else if (CLOSE.has(c) && --depth === 0) return i + 1;
  }
  throw new Error(`unbalanced brackets from ${text.slice(at, at + 40)}`);
}

/** The index of the next `;` outside brackets, comments and strings, from `at`. */
function statementEnd(text: string, at: number): number {
  for (let i = at; i < text.length; i++) {
    const c = text[i] ?? "";
    if (text.startsWith("/*", i)) i = text.indexOf("*/", i) + 1;
    else if (c === '"' || c === "'" || c === "`") i = text.indexOf(c, i + 1);
    else if (OPEN.has(c)) i = closing(text, i) - 1;
    else if (c === ";") return i;
  }
  return text.length;
}

/** A doc comment's text: one paragraph per blank line, lines joined. */
export function docText(comment: string): string {
  return comment
    .replace(/^\/\*\*/, "")
    .replace(/\*\/$/, "")
    .split("\n")
    .map((l) => l.replace(/^\s*\* ?/, "").trim())
    .join("\n")
    .trim()
    .split(/\n{2,}/)
    .map((p) => p.replace(/\s*\n\s*/g, " "))
    .join("\n\n");
}

const collapse = (s: string): string =>
  s
    .replace(/\/\*\*[\s\S]*?\*\//g, "")
    .replace(/\s+/g, " ")
    .replace(/([{([]) /g, "$1")
    .replace(/ ([})\]])/g, "$1")
    .replace(/;\s*([}])/g, "$1")
    .trim();

/** The members of an object type's body (the text between its braces). */
export function members(body: string): Member[] {
  const out: Member[] = [];
  let doc = "";
  let i = 0;
  while (i < body.length) {
    const rest = body.slice(i);
    const space = rest.match(/^\s+/);
    if (space) {
      i += space[0].length;
      continue;
    }
    if (rest.startsWith("/**")) {
      const end = body.indexOf("*/", i) + 2;
      doc = docText(body.slice(i, end));
      i = end;
      continue;
    }
    if (rest.startsWith("//")) {
      i = body.indexOf("\n", i) + 1 || body.length;
      continue;
    }
    const end = statementEnd(body, i);
    const text = body.slice(i, end).trim();
    i = end + 1;
    const m = text.match(/^(?:readonly\s+)?([A-Za-z_$][\w$]*)(\?)?\s*([(:])/);
    if (m === null) {
      doc = "";
      continue;
    }
    const [head = "", name = "", optional, kind] = m;
    const method = kind === "(";
    const raw = method ? "" : text.slice(head.length).trim();
    const type = collapse(method ? text.slice(head.length - 1) : raw);
    out.push({ name, optional: optional === "?", type, method, raw, doc });
    doc = "";
  }
  return out;
}

export class Declarations {
  readonly text: string;

  /** `files`: each declaration file's text by name, such as `index.d.ts`. */
  constructor(
    readonly files: Map<string, string>,
    readonly where: string,
  ) {
    this.text = [...files.values()].join("\n");
  }

  /** `packages/renderer/api.md`: a `## file.d.ts` heading and a ```ts block per file. */
  static fromApiMarkdown(markdown: string, where: string): Declarations {
    const files = new Map<string, string>();
    for (const m of markdown.matchAll(/^## (\S+\.d\.ts)\n+```ts\n([\s\S]*?)```/gm)) {
      files.set(m[1] ?? "", m[2] ?? "");
    }
    return new Declarations(files, where);
  }

  /** The declaration of `name`, with its doc comment, from `export` to its end. */
  private find(name: string): { doc: string; head: number; end: number } {
    const re = new RegExp(
      `(\\/\\*\\*(?:(?!\\*\\/)[\\s\\S])*\\*\\/\\s*)?export (?:declare )?(?:interface|type|function|const|class) ${name}\\b`,
    );
    const m = this.text.match(re);
    if (m === null || m.index === undefined) throw new Error(`${this.where}: no declaration of ${name}`);
    const head = m.index + (m[1]?.length ?? 0);
    const after = head + m[0].length - (m[1]?.length ?? 0);
    const kind = m[0].match(/(interface|type|function|const|class) \w+$/)?.[1];
    let end: number;
    if (kind === "interface" || kind === "class") {
      end = closing(this.text, this.text.indexOf("{", after));
    } else {
      end = statementEnd(this.text, after) + 1;
    }
    return { doc: m[1] === undefined ? "" : docText(m[1].trim()), head, end };
  }

  /** The doc comment on `name`'s declaration. */
  doc(name: string): string {
    return this.find(name).doc;
  }

  /** `name`'s declaration as written, doc comments inside it kept, its own left out. */
  source(name: string): string {
    const { head, end } = this.find(name);
    return this.text
      .slice(head, end)
      .replace(/^export (?:declare )?/, "")
      .replace(/^ {4}/gm, "  ");
  }

  /** The members of interface `name`, its `extends` left out. */
  interface(name: string): Member[] {
    const { head, end } = this.find(name);
    const text = this.text.slice(head, end);
    const open = text.indexOf("{");
    return members(text.slice(open + 1, -1));
  }

  /** The string members of union type `name`, each with the doc comment before it. */
  union(name: string): Array<{ value: string; doc: string }> {
    const { head, end } = this.find(name);
    const text = this.text.slice(head, end);
    return [...text.matchAll(/(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)?\s*\|?\s*"([^"]+)"/g)].map((m) => ({
      value: m[2] ?? "",
      doc: m[1] === undefined ? "" : docText(m[1]),
    }));
  }

  /**
   * Every name the entry module exports (in api.md, the `index.d.ts`
   * block): `export { a, type B } from`, `export type { … } from`,
   * `export declare function …`.
   */
  exported(): string[] {
    const scope = this.files.get("index.d.ts");
    if (scope === undefined) throw new Error(`${this.where}: no index.d.ts`);
    const names = new Set<string>();
    for (const m of scope.matchAll(/export (?:type )?\{([^}]*)\} from/g)) {
      for (const part of (m[1] ?? "").split(",")) {
        const name = part.trim().replace(/^type\s+/, "");
        if (name !== "") names.add(name);
      }
    }
    for (const m of scope.matchAll(/export declare (?:function|const|class) (\w+)/g)) {
      names.add(m[1] ?? "");
    }
    return [...names].toSorted();
  }
}
