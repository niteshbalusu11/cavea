// One page's Markdown to HTML, with Bun's own parser (`Bun.markdown`, GitHub
// flavour) and a few conventions of this site on either side of it:
//
// - `{{name}}` is a variable from `site.json` (an unknown one fails the build);
// - a fence's info string may carry `key=value` words after the language:
//   `file=compose.yaml` captions the block with that file name, and `qs=…`
//   marks a block the quick-start check runs (`quickstart/check.ts`);
// - a block quote that starts with **Note**, **Warning** or **Not yet** is a
//   callout;
// - links to `.md` files become links to their `.html` pages, so the sources
//   read correctly on GitHub too.

export interface Heading {
  level: number;
  id: string;
  text: string;
}

export interface Rendered {
  title: string;
  description: string;
  html: string;
  headings: Heading[];
}

export interface Fence {
  lang: string;
  meta: Record<string, string>;
  code: string;
  /** 1-based line of the opening fence. */
  line: number;
}

const FENCE = /^(\s*)(`{3,}|~{3,})(.*)$/;

/** The words after a fence's language, `key=value` each. */
export function fenceMeta(info: string): { lang: string; meta: Record<string, string> } {
  const [lang = "", ...words] = info.trim().split(/\s+/);
  const meta: Record<string, string> = {};
  for (const word of words) {
    const at = word.indexOf("=");
    if (at > 0) meta[word.slice(0, at)] = word.slice(at + 1);
  }
  return { lang, meta };
}

/** Every fenced block of a Markdown source, with its info string's words. */
export function fences(source: string): Fence[] {
  const out: Fence[] = [];
  const lines = source.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const open = lines[i]?.match(FENCE);
    if (!open) continue;
    const [, indent = "", marks = "", info = ""] = open;
    const body: string[] = [];
    let j = i + 1;
    for (; j < lines.length; j++) {
      const line = lines[j] ?? "";
      if (line.trim().startsWith(marks) && line.trim().replace(/[`~]/g, "") === "") break;
      body.push(line.startsWith(indent) ? line.slice(indent.length) : line.trimStart());
    }
    out.push({ ...fenceMeta(info), code: `${body.join("\n")}\n`, line: i + 1 });
    i = j;
  }
  return out;
}

/** `{{name}}` replaced from `vars`; throws naming every unknown one. */
export function substitute(source: string, vars: Record<string, string>, where: string): string {
  const unknown = new Set<string>();
  const out = source.replace(/\{\{\s*([\w-]+)\s*\}\}/g, (_, name: string) => {
    const value = vars[name];
    if (value === undefined) unknown.add(name);
    return value ?? "";
  });
  if (unknown.size > 0) throw new Error(`${where}: unknown variable ${[...unknown].join(", ")}`);
  return out;
}

export function escape(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Text of an HTML fragment: tags dropped, entities decoded. */
export function plain(html: string): string {
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}

/** Fence info strings reduced to the language, with a caption before a block that names a file. */
function captions(source: string): string {
  const lines = source.split("\n");
  const out: string[] = [];
  let open: string | null = null;
  for (const line of lines) {
    const m = line.match(FENCE);
    if (open === null && m) {
      const [, indent = "", marks = "", info = ""] = m;
      const { lang, meta } = fenceMeta(info);
      if (meta.file !== undefined) {
        out.push(`${indent}<p class="file"><span>${escape(meta.file)}</span></p>`, "");
      }
      out.push(`${indent}${marks}${lang}`);
      open = marks;
      continue;
    }
    if (open !== null && line.trim().startsWith(open) && line.trim().replace(/[`~]/g, "") === "") {
      open = null;
    }
    out.push(line);
  }
  return out.join("\n");
}

const CALLOUTS: Record<string, string> = { Note: "note", Warning: "warning", "Not yet": "later" };

export function render(source: string, page: string): Rendered {
  let html = Bun.markdown.html(captions(source), { headings: { ids: true } });

  // Links between pages: `x.md#y` → `x.html#y`.
  html = html.replace(/href="([^"#:]+)\.md(#[^"]*)?"/g, (_, path: string, hash = "") => {
    return `href="${path}.html${hash}"`;
  });

  // Callouts.
  html = html.replace(
    /<blockquote>\s*<p><strong>(Note|Warning|Not yet)<\/strong>:?\s*([\s\S]*?)<\/blockquote>/g,
    (_, kind: string, rest: string) =>
      `<aside class="callout ${CALLOUTS[kind]}"><p><strong>${kind}:</strong> ${rest}</aside>`,
  );

  // Wide things scroll inside their own box on a phone.
  html = html
    .replace(/<table>/g, '<div class="table" tabindex="0"><table>')
    .replace(/<\/table>/g, "</table></div>")
    .replace(/<pre>/g, '<div class="code"><pre tabindex="0">')
    .replace(/<\/pre>/g, "</pre></div>");

  // Headings: a link to the section after each h2 and h3.
  const headings: Heading[] = [];
  html = html.replace(
    /<h([1-4]) id="([^"]+)">([\s\S]*?)<\/h\1>/g,
    (_, level: string, id: string, inner: string) => {
      const n = Number(level);
      headings.push({ level: n, id, text: plain(inner) });
      if (n === 1 || n === 4) return `<h${n} id="${id}">${inner}</h${n}>`;
      return `<h${n} id="${id}">${inner}<a class="anchor" href="#${id}" aria-label="Link to this section">#</a></h${n}>`;
    },
  );

  const h1 = headings.find((h) => h.level === 1);
  if (h1 === undefined) throw new Error(`${page}: no level-1 heading`);
  if (headings.filter((h) => h.level === 1).length > 1) {
    throw new Error(`${page}: more than one level-1 heading`);
  }
  const first = html.match(/<\/h1>\s*<p>([\s\S]*?)<\/p>/)?.[1] ?? "";
  return { title: h1.text, description: plain(first).slice(0, 200), html, headings };
}
