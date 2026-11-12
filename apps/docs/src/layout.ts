// The page around a page's content: header with search, the navigation, the
// content, the section list, previous and next. Links are relative to the
// page, so the built site works from any directory of any static host, and
// from the file system. Readable without JavaScript: search is the only
// part that needs it, and it stays hidden without it.

import { escape, type Heading } from "./markdown.ts";

export interface NavSection {
  section: string;
  pages: Array<{ path: string; title: string }>;
  planned: string | undefined;
}

export interface LayoutInput {
  site: string;
  path: string;
  title: string;
  description: string;
  content: string;
  headings: Heading[];
  nav: NavSection[];
  published: boolean;
  prev: { path: string; title: string } | undefined;
  next: { path: string; title: string } | undefined;
  absolute?: boolean;
}

export function layout(p: LayoutInput): string {
  const depth = p.path.split("/").length - 1;
  const root = p.absolute === true ? "/" : "../".repeat(depth);
  const link = (to: string): string => `${root}${to}`;

  const nav = p.nav
    .filter((s) => s.pages.length > 0)
    .map((s) => {
      const items = s.pages
        .map((page) => {
          const current = page.path === p.path ? ' aria-current="page"' : "";
          return `<li><a href="${link(page.path)}"${current}>${escape(page.title)}</a></li>`;
        })
        .join("");
      return `<h2>${escape(s.section)}</h2><ul>${items}</ul>`;
    })
    .join("\n");

  const toc = p.headings.filter((h) => h.level === 2);
  const onPage =
    toc.length > 1
      ? `<nav class="toc" aria-labelledby="toc-title"><h2 id="toc-title">On this page</h2><ul>${toc
          .map((h) => `<li><a href="#${h.id}">${escape(h.text)}</a></li>`)
          .join("")}</ul></nav>`
      : "";

  const pager =
    p.prev !== undefined || p.next !== undefined
      ? `<nav class="pager" aria-label="Previous and next pages">${
          p.prev !== undefined
            ? `<a class="prev" href="${link(p.prev.path)}" rel="prev"><span>Previous</span>${escape(p.prev.title)}</a>`
            : "<span></span>"
        }${
          p.next !== undefined
            ? `<a class="next" href="${link(p.next.path)}" rel="next"><span>Next</span>${escape(p.next.title)}</a>`
            : ""
        }</nav>`
      : "";

  const unreleased = p.published
    ? ""
    : `<p class="unreleased" role="note">Cavea is not released yet. Until it is, build the server image and the seat map from the repository: <a href="${link("before-release.html")}">Before the first release</a>.</p>`;

  const title = p.path === "index.html" ? p.site : `${p.title} · ${p.site}`;
  return `<!doctype html>
<html lang="en-GB">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escape(title)}</title>
${p.description === "" ? "" : `<meta name="description" content="${escape(p.description)}">\n`}<meta name="color-scheme" content="light dark">
<link rel="stylesheet" href="${link("style.css")}">
<link rel="icon" href="${link("favicon.svg")}" type="image/svg+xml">
<script src="${link("site.js")}" defer></script>
</head>
<body data-root="${root}">
<a class="skip" href="#content">Skip to the content</a>
<header class="top">
<a class="brand" href="${link("index.html")}">Cavea <span>docs</span></a>
<form class="search" role="search" action="${link("index.html")}" hidden>
<label for="search-input" class="visually-hidden">Search the documentation</label>
<input id="search-input" type="search" placeholder="Search" autocomplete="off" spellcheck="false" aria-controls="search-results" aria-describedby="search-hint">
<span id="search-hint" class="visually-hidden">Results appear below as you type.</span>
<div id="search-results" class="results" role="region" aria-live="polite" aria-label="Search results" hidden></div>
</form>
</header>
<div class="shell">
<details class="menu" open>
<summary>Menu</summary>
<nav class="side" aria-label="Documentation">
${nav}
</nav>
</details>
<main id="content" tabindex="-1">
${unreleased}<article>
${p.content}
</article>
${pager}
</main>
${onPage}
</div>
</body>
</html>
`;
}
