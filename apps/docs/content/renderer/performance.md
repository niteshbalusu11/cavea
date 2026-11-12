# Loading and performance

How to make the seat map appear sooner, how to bundle it, and the Content Security Policy it needs. By the end you will know what to add to your page and your build, and what each costs.

## What the browser downloads

| File | What it is | Size, compressed with brotli |
|---|---|---|
| The package's JavaScript | The API, the built-in controls, the adapters | about 26 KB |
| `cavea_web_bg.wasm` | The renderer | about 104 KB |
| `glyphs.png` | The seat labels' font | about 9 KB |
| The chart, `.ccf` | Your venue, compiled | Grows with the venue: about 7 KB for 2,000 seats, 54 KB for 20,000, 181 KB for 100,000 (the repository's synthetic venues) |

The package's figures are from the build of the version this site describes; the repository's build fails if the three together pass 200 KB. Bundlers name each file with a content hash, so a CDN or your server can cache them for a long time.

## Preload

`preloadSeatMap` starts fetching and compiling the WebAssembly and the glyph atlas, and fetching a chart, before the map's container exists: as soon as you know the buyer is heading for the seat map (on hover of a "Choose seats" link, say). A `createSeatMap` after it uses what it fetched.

<!-- declaration: preloadSeatMap -->

```ts check=types
import { preloadSeatMap } from "{{renderer_package}}";
#hide declare const link: HTMLAnchorElement;

link.addEventListener("pointerenter", () => void preloadSeatMap(), { once: true });
```

It never rejects, is safe to call often, and does nothing where there is no DOM. Measured on the developers' machine, in headless Chromium with software WebGL, with a 100,000-seat chart and the files served from the same machine (so no network time), the map created 3 s after the page started: the first frame came about 50 ms after `createSeatMap` with preloading and about 150 ms without. That is one machine's figure, not a promise; over a real network the saving grows by the download time.

**In the page's `<head>`**, preload hints start the downloads before any script runs: `<link rel="preload" href="…/cavea_web_bg.wasm" as="fetch" type="application/wasm" crossorigin>`, the same for `glyphs.png` and the chart without the `type`. Use `as="fetch"` with `crossorigin`, as the map fetches them; any other `as`, or no `crossorigin`, downloads the file twice. A bundler renames the files with a hash: take the names from its manifest, or serve the two files from a fixed place and give it as `assets.baseUrl`.

## Other advice

- **Size the container** before creating the map. It follows later resizes.
- **Background images** never delay the map: they are fetched after the first frame and appear when ready.
- **`maxDpr`** caps the pixel ratio the map draws at (2 on touch screens, 3 otherwise). Lower it on pages that must draw cheaply on high-density screens.
- **Watching performance:** `onReady`'s `marks` say where the start's time went, and `onMetrics` reports frame times once a second ([Callbacks](callbacks.md#ready-and-metrics)).

## Bundlers

| Tool | What it needs |
|---|---|
| Vite, build and dev server | Nothing |
| webpack 5 | Nothing |
| Next.js (App Router, Turbopack) | Nothing, for `@cavea/react` in a client component or `import("{{renderer_package}}")` in an effect |
| Rollup | Copy `cavea_web_bg.wasm` and `glyphs.png` from the package beside the output, and `@rollup/plugin-node-resolve` as for any package |
| esbuild | `--format=esm`, and copy the two files beside the output |
| No bundler | `cavea.min.js` by a script tag, or `index.js` as a module, with the two files beside it |

Instead of copying, any of them can serve the two files from a place of your choosing, named by `assets.baseUrl`. Each set-up in the table is built and loaded in a browser by the repository's compatibility checks.

## Content Security Policy

The smallest policies that work, found by starting from nothing and adding one source at a time, then taking each away again:

| Set-up | Policy |
|---|---|
| The package bundled with your page, files on your page's origin, your own adapter | `default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'` |
| `caveaServer` with the server on another origin | The above, with `connect-src 'self' https://seats.example.com wss://seats.example.com` |
| `cavea.min.js` from a CDN | `default-src 'none'; script-src 'self' https://cdn.example.com 'wasm-unsafe-eval'; connect-src 'self' https://cdn.example.com` |

- `'wasm-unsafe-eval'` lets the browser compile WebAssembly. Without it `createSeatMap` rejects at once with `wasm_unavailable`.
- `connect-src` needs every origin the map fetches from: the files, the chart, the server, and the server's `wss:` origin for the WebSocket, which an `https:` source does not cover. Without the `wss:` origin the map still works, over Server-Sent Events.
- Nothing else: no `img-src`, `style-src`, `font-src`, `worker-src` or `'unsafe-inline'`. The controls' styles are a constructed stylesheet, which `style-src` does not govern; Safari before 16.4 lacks those and gets a `<style>` element, which a policy without `style-src 'unsafe-inline'` blocks, leaving the controls unstyled.
- Trusted Types: the map builds no HTML or script from strings, so `require-trusted-types-for 'script'` needs nothing from it.

## Server-side rendering

Importing the package on a server is safe: nothing touches `window` until `createSeatMap` runs, which rejects with `not_browser` there. Create the map in the browser, after mount. The [React guide](../guides/react.md#nextjs) has the Next.js set-up.

## Browsers

The map needs WebAssembly and WebGL2: current Chrome, Edge, Firefox and Safari, and Safari on iOS 15 and later. Where either is missing, `createSeatMap` rejects ([Errors](errors.md)); a Canvas 2D fallback and a list view are planned.
