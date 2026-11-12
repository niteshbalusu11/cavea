# Errors

What the seat map does when something fails, what the buyer sees, and what your page is told. By the end you will know which errors stop the map and which it recovers from by itself.

Every error the map reports is a `CaveaError`:

<!-- declaration: CaveaError -->

A **fatal** error rejects `createSeatMap`: the map is not shown. After `createSeatMap` has resolved, every error is recoverable, goes to `onError`, and never reaches your page as an exception.

## Codes

<!-- union: CaveaErrorCode -->

Fatal at start: `invalid_option`, `not_browser`, `wasm_unavailable`, `webgl_unavailable`, `chart_fetch_failed`, `bad_chart`, `state_connect_failed` and `aborted`. `chart_fetch_failed`, `wasm_unavailable` and `state_connect_failed` are also reported as recoverable while the error state offers the buyer a retry (below).

Errors from the Cavea server (a hold refused, a rate limit) are a different list: the [server's error codes](../reference/errors.md).

## What the buyer sees

| Situation | Code | The buyer sees | Your page gets |
|---|---|---|---|
| Invalid options | `invalid_option`, fatal | Nothing: the map is not mounted | A rejection |
| Not in a browser (server-side rendering) | `not_browser`, fatal | Nothing | A rejection |
| The chart, the WebAssembly or the first state fails for a while, then loads | none | The loading skeleton throughout | Nothing; `createSeatMap` resolves |
| …and still fails after three retries | `chart_fetch_failed`, `wasm_unavailable` or `state_connect_failed` | "The seat map could not be loaded." and **Try again** | `onError`; the promise stays pending and resolves when a retry succeeds |
| …the same with `ui.loading: false` | The same codes, fatal | Nothing: the map is removed | A rejection |
| Offline before the map has drawn | none | "You are offline. The seat map will load when you are back online." | Nothing until a try fails online |
| The server no longer has the chart, the event is unknown, or the key is refused | `chart_fetch_failed` or `state_connect_failed`, fatal | Nothing | A rejection, without retries |
| The chart file is not a compiled chart | `bad_chart`, fatal | Nothing | A rejection |
| No WebAssembly, or a Content Security Policy blocks it | `wasm_unavailable`, fatal | Nothing | A rejection |
| No WebGL2 | `webgl_unavailable`, fatal | Nothing | A rejection |
| Your page aborts `signal` before the map is ready | `aborted`, fatal | Nothing | A rejection |
| The glyph atlas fails | `glyph_atlas_failed` | Seats without labels | `onError` |
| A background image fails | `image_failed` | The map without that image | `onError`, once per image |
| The GPU context is lost for 5 s | `context_lost` | "The seat map cannot be drawn right now." until the browser restores it | `onError`, once per loss |
| The live connection drops | `state_disconnected` | "Live seat updates were interrupted. Reconnecting…"; the map stays usable | `onError` at the first drop |
| A hold fails for network reasons | `hold_failed` | The seat is dropped, with a notice | `onHoldFailed`, `onError` |

While the loading skeleton or the error state shows, the zoom controls and legend are hidden. With no WebGL2 or no WebAssembly there is no fallback yet: the Canvas 2D renderer and the list view are not built, so your page should show its own message when `createSeatMap` rejects with `webgl_unavailable` or `wasm_unavailable`.

```ts check=types
import { CaveaError, createSeatMap, staticState } from "{{renderer_package}}";
#hide declare const container: HTMLElement;

try {
  await createSeatMap(container, { chart: { url: "/charts/grand-hall.ccf" }, state: staticState() });
} catch (error) {
  if (error instanceof CaveaError && (error.code === "webgl_unavailable" || error.code === "wasm_unavailable")) {
    container.textContent = "Your browser cannot show the seat map. Call the box office on 01632 960000.";
  } else {
    throw error;
  }
}
```

## Retries

The WebAssembly, the chart, the first state and the glyph atlas are each tried again after a failure that may pass: three times, after about 1, 2 and 4 s, with some randomness so that many pages do not retry together. While the page is hidden or the browser is offline, the next try waits. Never tried again, because they cannot succeed: an HTTP status from 400 to 499 other than 408 and 429, WebAssembly that does not compile or that a Content Security Policy refuses, a chart that does not decode, and a browser without WebAssembly or WebGL2.

The live connection has its own reconnection: [the live connection](adapters.md#the-live-connection).
