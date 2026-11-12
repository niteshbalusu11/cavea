# The seat map

This section is the reference for `{{renderer_package}}`, the seat map a buyer's page shows. By the end of this page you will know how to add it to a page, what `createSeatMap` does, and where each part of its interface is described.

The seat map draws a [chart](../concepts/charts.md) with WebGL2, from WebAssembly, and shows each seat as available, unavailable or selected. Where that state comes from is a [state adapter](adapters.md): your Cavea server, your own backend, or a fixed list.

## Install

```sh
npm install {{renderer_package}}
```

pnpm, yarn and bun work the same way. The package has no runtime dependencies. Besides its JavaScript it ships two files the map fetches at run time, `cavea_web_bg.wasm` (the renderer) and `glyphs.png` (the seat labels' font); Vite, webpack and Next.js copy them for you, Rollup and esbuild need a copy step ([Loading and performance](performance.md#bundlers)).

Without a bundler, load the script-tag build, `cavea.min.js`, which sets `window.Cavea` to the same functions. The [quick start](../quick-start.md#7-write-the-page) uses it.

> **Not yet** The package is not published. Until it is, build it from the repository: [Before the first release](../before-release.md#build-the-seat-map).

## Draw a map

```ts check=types
import { caveaServer, createSeatMap } from "{{renderer_package}}";

const container = document.getElementById("map");
if (container === null) throw new Error("no #map element");

const map = await createSeatMap(container, {
  state: caveaServer({
    baseUrl: "https://seats.example.com",
    publishableKey: "pk_paste-your-publishable-key-here",
    event: "opening-night",
  }),
  selection: { max: 6 },
  onSelectionChange: (selection) => {
    console.log(selection.objects.map((seat) => seat.label), selection.holdToken);
  },
});
```

`createSeatMap(element, options)`:

1. checks the options, and rejects at once if one is invalid;
2. starts fetching the WebAssembly, the glyph atlas, the chart and the first seat state together;
3. mounts a canvas and the built-in controls in a shadow root of `element`, filling it;
4. resolves with the map once the chart is decoded and the first state applied.

Give the element a size before you create the map, as the `div` above has from its page's CSS: the map fills the element and follows later resizes. An element hidden with `display: none` works; the map draws when it gets a size.

<!-- declaration: createSeatMap -->

The promise rejects only at start, with a fatal [`CaveaError`](errors.md): invalid options, no WebAssembly or WebGL2, a chart or first state that cannot be had. Once it has resolved, nothing throws into your page: problems go to `onError`.

## What is where

| Page | What it covers |
|---|---|
| [Options](options.md) | Everything `createSeatMap` takes: data, selection rules, behaviour, the built-in interface, loading |
| [Callbacks](callbacks.md) | What the map tells your page, and the objects it reports |
| [Methods](methods.md) | What your page can ask of a map once it is ready |
| [State adapters](adapters.md) | `caveaServer`, `staticState`, and `customState` for your own backend |
| [Theming and text](theming.md) | Themes, colour tokens, styling the controls, messages and languages |
| [Errors](errors.md) | Error codes, what the buyer sees when something fails, and retries |
| [Loading and performance](performance.md) | Preloading, bundlers, Content Security Policy, server-side rendering |
| [Accessibility](accessibility.md) | Keyboard use, screen readers, contrast, and what is not built yet |

For React there is a component, `<SeatMap>`: see the [React guide](../guides/react.md).

## Development builds

In a development build the map writes warnings to the browser's console: an unknown option, an option that is not supported yet, a method called on a destroyed map, and errors when you gave no `onError`. Each warning is written once per map. A development build is any build in which `process.env.NODE_ENV` is not `"production"`; bundlers set it in production builds, and a page with no bundler counts as development. In production the map writes nothing to the console.

## Version

`loadRenderer()` loads the WebAssembly as a map would and reports the renderer's version, for a support request or a diagnostics page.

<!-- declaration: loadRenderer -->
