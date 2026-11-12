# The public documentation site

What a developer at a ticketing company reads to evaluate Cavea and to get started, and what an operator reads to run it. The internal specifications are in `docs/` at the repository's root and are not this. Decision: `docs/DECISIONS.md` D-83.

## Layout

| Path | What it is |
|---|---|
| `content/**/*.md` | The pages, written by hand. A page's path is its URL: `content/concepts/holds.md` is `concepts/holds.html`. |
| `site.json` | The navigation (every page, in order, by section; sections still to come as `planned`), the variables pages use as `{{name}}`, and `published`. |
| `src/build.ts` | The build: Markdown to HTML with `Bun.markdown`, the layout, the search index, the checks. |
| `src/api.ts` | The server API reference, generated from `crates/cavea-server/openapi.json` at build time. |
| `src/errors.ts` | The check that `content/reference/errors.md` matches the server's error codes and reasons. |
| `static/` | Copied to the site as it is: the stylesheet, the script (search, copy buttons), the favicon, the screenshots. |
| `quickstart/check.ts` | Runs the quick start end to end; also retakes the screenshots. |

## Build and preview

```sh
bun apps/docs/src/build.ts                 # writes apps/docs/dist/
python3 -m http.server -d apps/docs/dist 0 # or any static server; the pages also open from the file system
```

`--var name=value` overrides a variable of `site.json` for one build, such as `--var image=cavea-server:local`.

## Writing

- Every page starts with what the reader will have or know at the end. A task page is numbered steps, each saying what the reader should see. A reference page is complete and scannable.
- Plain, direct sentences in British English. Name things exactly as the product does: the console's labels in bold, as they appear on screen; the API's fields in code. Define a term where it first appears and link to its concept page.
- Links between pages point at the `.md` file (`[Holds](../concepts/holds.md)`), so they work on GitHub; the build makes them `.html`. Anchors are the heading's text in lowercase with dashes; a heading that is only a code span (an error code) is its own anchor.
- A block quote starting with **Note**, **Warning** or **Not yet** is a call-out.
- A fenced block may carry words after its language: `file=compose.yaml` captions it with a file name; `qs=…` marks a quick-start block the check runs (`qs=dir`, `up`, `link`, `api`, `down`, and `file` for files).
- Keys, tokens and passwords in examples are obviously fake (`sk_paste-the-secret-key-here`), never 64 hexadecimal digits. Names, emails and venues are invented (`example.com`, `example.org`).
- Never paste the specifications: write the customer's page from them, without their history and rationale.

## Adding a page

Write it under `content/`, add it to a section's `pages` in `site.json`, and build: the build fails while a page is missing from the navigation or a link is broken.

## The quick start

The quick start is run by machine from its own source: change it and its check together, and run the check before filing.

```sh
cargo xtask build-web && (cd packages/renderer && bun run build)   # the seat map, until it is published
nix develop -c bun apps/docs/quickstart/check.ts [--image TAG] [--path console|api|both] [--screenshots]
```

Without `--image` it builds the server image from the repository first (and removes it at the end). `--screenshots` retakes the three screenshots in `static/images/` (light theme, WebP, about 220 KB together) from the console path. Every bold phrase in the quick start must be a label the console shows, or one of the page's own call-outs listed in the check.

## Publishing

`site.json` names the server image and the seat map's script URL as they will be published, and `"published": false` puts a notice on every page that points to "Before the first release". At the first release: set the real names and version, set `published` to `true`, delete `content/before-release.md` and its entry in `site.json`.

The output is static files. To deploy, copy `apps/docs/dist/` to a static host: GitHub Pages, Cloudflare Pages, Netlify, or a bucket behind a CDN, at the root of a host name such as `docs.example.com` (the 404 page links from the root). The host and domain are the owner's choice.
