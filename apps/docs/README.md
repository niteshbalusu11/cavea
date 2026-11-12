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
| `src/reference.ts`, `src/declarations.ts` | What the written pages print from the product's source (directives, below), and the checks on it. |
| `src/samples.ts` | Writes the pages' `check=types` samples to `samples/` (gitignored) for `bun run typecheck` in `apps/docs`. |
| `static/` | Copied to the site as it is: the stylesheet, the script (search, copy buttons), the favicon, the screenshots. |
| `quickstart/check.ts` | Runs the quick start end to end; also retakes the screenshots. |
| `deployment/check.ts` | Runs the deployment guide's compose file, Caddyfile, setup, reconciler, backup and restore. |

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

## Code samples are checked

Every block in a code or configuration language (`ts`, `tsx`, `js`, `jsx`, `html`, `yaml`, `nginx`, `caddyfile`) says how it is checked, or the build fails. Shell commands, JSON and `text` output are exempt.

| Mark | Checked by |
|---|---|
| `check=types` | `bun run typecheck` in `apps/docs`: `src/samples.ts` writes each such block to `samples/`, and `tsc -p tsconfig.samples.json` compiles them against the workspace's `@cavea/renderer`, `@cavea/react` and `@cavea/server-sdk` as a customer's strict project would. A line starting `#hide ` is compiled but not shown: declare what a fragment assumes there (`#hide declare const container: HTMLElement;`). A `js` block is compiled as TypeScript |
| `check=file:<path>` | The build: the block must equal that repository file, which its own check runs |
| `check=run:<name>` | `apps/docs/<name>/check.ts`, which takes every block so marked from the pages' source and fails unless it runs each one (`run:deployment`, `run:quickstart`) |
| `qs=…` | The quick start's own marks (above) |

## Generated parts of written pages

A line of its own in a page prints something from the product's source, so it cannot drift:

| Line | Prints |
|---|---|
| `<!-- members: SeatMapOptions options -->` | An interface of `@cavea/renderer` (from `packages/renderer/api.md`) as a table: `options` leaves out callbacks, `callbacks` keeps only them; `react:SeatMapProps` reads `packages/react/src/index.ts` |
| `<!-- union: CaveaErrorCode -->` | A string union, a row per member |
| `<!-- declaration: Snapshot -->` | A declaration as TypeScript |
| `<!-- messages -->`, `<!-- theme-defaults -->` | The renderer's English messages; its built-in schemes' colours |
| `<!-- shortcuts -->` | The designer's keyboard shortcuts, from `apps/console/src/designer/shortcuts.ts` |
| `<!-- metrics -->` | The server's Prometheus metrics, from the Rust that writes them |

The build fails when a member or union member it prints has no doc comment (add one to the package's declaration and run `bun run build --update-api` in `packages/renderer`), when a name `@cavea/renderer` exports is printed by no page, when the theming page's token table differs from the tokens the renderer reads, when the settings page's table differs from the variables `crates/cavea-server/src/config.rs` reads, and when a page names a metric the server does not have. A table row whose first cell starts with a code span gets that name as its id and its own search entry, so `renderer/options.html#selection.max` and a search for `CAVEA_TRUSTED_PROXIES` both work.

## Adding a page

Write it under `content/`, add it to a section's `pages` in `site.json`, and build: the build fails while a page is missing from the navigation or a link is broken.

## The quick start

The quick start is run by machine from its own source: change it and its check together, and run the check before filing.

```sh
cargo xtask build-web && (cd packages/renderer && bun run build)   # the seat map, until it is published
nix develop -c bun apps/docs/quickstart/check.ts [--image TAG] [--path console|api|both] [--screenshots]
```

Without `--image` it builds the server image from the repository first (and removes it at the end). `--screenshots` retakes the three screenshots in `static/images/` (light theme, WebP, about 220 KB together) from the console path. Every bold phrase in the quick start must be a label the console shows, or one of the page's own call-outs listed in the check.

## The deployment guide

`content/deployment/` is run by machine like the quick start: change a marked block and the check together, and run the check before filing a change to the image, the compose files, start-up or the server's settings.

```sh
cargo xtask build-web && (cd packages/renderer && bun run build)   # the buyer's page in the check
nix develop -c bun apps/docs/deployment/check.ts [--image TAG]
```

It runs the pages' `compose.yaml` and `Caddyfile` under a project name, port and private subnet of its own, with `localhost` and `tls internal` (a certificate Caddy makes at run time) in place of the page's host name; creates the owner from the logged setup link in headless Chromium and makes a cookie-authenticated change, through the proxy; loads a buyer's page from another origin with the WebSocket transport forced and checks that a seat held by the backend turns unavailable on it; runs the reconciler; then runs the backup page's dump, deletes the database volume, restores, and checks that a booking and an uploaded image survived. About 30 s with an image; without `--image` it builds one first. It removes every container, volume and network it made, the image it built, and the Caddy image if it pulled it.

## Publishing

`site.json` names the server image and the seat map's script URL as they will be published, and `"published": false` puts a notice on every page that points to "Before the first release". `site.json`'s `image`, `renderer_package`, `server_sdk_package` and `repository` are also what `cargo xtask release` publishes to (D-91): it refuses while the image's registry is the `example` placeholder or a name disagrees with the packages. `cargo xtask release prepare <version>` sets `version`, the image's tag and the script's URL and sets `published` to `true`. At the first release, also set the real registry in `image` and delete `content/before-release.md` and its entry in `site.json`.

The output is static files. To deploy, copy `apps/docs/dist/` to a static host: GitHub Pages, Cloudflare Pages, Netlify, or a bucket behind a CDN, at the root of a host name such as `docs.example.com` (the 404 page links from the root). The host and domain are the owner's choice.
