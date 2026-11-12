# 05 · Web SDK

**Status:** Draft v0.1 · 2026-11-10

The public JavaScript surface of the renderer. Everything here is semver-stable from 1.0.

## 1. Packages

| Package | Contents |
|---|---|
| `@cavea/renderer` | Framework-agnostic renderer: TypeScript API, WASM, font atlas, default styles |
| `@cavea/react` | React component, `<SeatMap>` (§11) |
| `@cavea/vue`, `@cavea/svelte` | Thin wrappers (props in, events out). Not yet |
| `@cavea/designer-embed` | Helper for embedding the designer in an iframe |
| `@cavea/manager-embed` | Helper for embedding the event manager |
| `@cavea/server-sdk` | Node client for the server's REST API (never used in the browser; §15) |

**What a customer installs** (D-58). `@cavea/renderer` is built by `bun run build` in `packages/renderer` into `dist/`, which is the package (`bun pm pack` in it; publishing is OPS-001). Every file is at the top of the package:

| File | What |
|---|---|
| `index.js` | The public API as one ES module, not minified, with `process.env.NODE_ENV` left for the customer's bundler (§3.6). `exports["."]`, with `index.d.ts` |
| `index.d.ts` and the declarations it imports | The public types; `packages/renderer/api.md` is all of them as one file, so an API change shows in a pull request's diff (the build fails while it is stale) |
| `cavea.min.js` | The script-tag build: a classic script, minified, production, that sets `window.Cavea` to the public API |
| `cavea_web_bg.wasm`, `glyphs.png` | The renderer and the seat labels' atlas, beside both scripts (`OFL.txt` is the atlas font's licence) |

The package has no runtime dependencies, `"type": "module"`, `"sideEffects": false`, and exports the four files above by name (`@cavea/renderer/cavea.min.js`, `…/cavea_web_bg.wasm`, `…/glyphs.png`) for hosts that copy or preload them. Importing it touches no browser global.

**Where the WASM and the atlas come from.** Both scripts name them `new URL("./cavea_web_bg.wasm", import.meta.url)` and `new URL("./glyphs.png", import.meta.url)`: beside the script that runs. For `index.js`, a bundler that understands the pattern copies and renames them (§13); without a bundler, an ES module served with them finds them beside it. `cavea.min.js` reads its own URL from `document.currentScript` while it runs, so it finds them beside itself on any origin; loaded where there is no current script (as a module, or evaluated by a loader), it uses the page's URL. `assets.baseUrl` (§3.5) overrides both: the two files under another base, relative to the page. `assets.wasm` brings the WASM itself.

**The script-tag build.** For sites without a bundler:

```html
<script src="https://cdn.example.com/cavea/0.1.0/cavea.min.js"
        integrity="sha384-…" crossorigin="anonymous"></script>
<script>
  Cavea.createSeatMap(document.getElementById('map'), { chart: { url: '…' }, state: Cavea.customState({ … }) });
</script>
```

`window.Cavea` has every export of `@cavea/renderer` (`createSeatMap`, `preloadSeatMap`, `customState`, `staticState`, `caveaServer`, `CaveaError`, `loadRenderer`). It is a production build: development warnings (§3.6) are off; a developer who wants them uses `index.js`. From a CDN on another origin, the CDN sends `Access-Control-Allow-Origin` for the script (needed by `integrity` with `crossorigin`) and for the WASM and the atlas (fetched with `fetch()` in CORS mode).

**Integrity.** The script's own hash goes in its `integrity` attribute (`bun run build` prints it). The page cannot put a hash on what the script fetches, so the script pins them itself: `cavea.min.js` carries the SHA-384 of the WASM and the atlas it was built with and fetches both with `integrity`, so a pinned script pins the files it loads, wherever `assets.baseUrl` points; a changed WASM fails to load (`wasm_unavailable`, fatal; a changed atlas, `glyph_atlas_failed`). A host's own `assets.wasm` is not checked. A `<link rel="preload">` for either file needs the same `integrity` attribute, or the browser fetches it twice. `index.js` pins nothing: a bundler may serve its own copies (and an image optimiser may rewrite the atlas).

**Built today** (WEB-001 to WEB-018, WEB-020 to WEB-025): `createSeatMap`, `staticState`, `customState`, `caveaServer`, `CaveaError` and their types, from `@cavea/renderer`, which has no runtime dependencies, with the built-in interface of §7 (zoom controls, legend, tooltip, toasts, loading and error states), prices (§3.1), the message catalogue (§3.4) and theme tokens as CSS custom properties (§8). Features this document describes but that are not built yet are marked "not yet" with the task that builds them. Options and callbacks that are not built are left out of the TypeScript types, so a typed build fails on them; passed anyway, they are ignored with a development warning (§3.6). `loadRenderer()` loads the shared WASM and reports its `version()`; nothing else below the public API is exported.

## 2. Quick start

```ts
import { createSeatMap, caveaServer } from '@cavea/renderer';

const map = await createSeatMap(document.getElementById('map')!, {
  chart: { url: 'https://tickets.example.com/cavea/v1/public/charts/9f2c….ccf' },
  state: caveaServer({
    baseUrl: 'https://tickets.example.com/cavea',
    publishableKey: 'pk_live_…',
    event: 'hamlet-2027-03-14',
    session: 'start',
  }),
  pricing: [
    { category: 'vip',      price: 9500, ticketTypes: [{ key: 'adult', price: 9500 }, { key: 'child', price: 6000 }] },
    { category: 'standard', price: 4500 },
  ],
  formatPrice: (minor) => new Intl.NumberFormat('en-GB', { style: 'currency', currency: 'GBP' }).format(minor / 100),
  selection: { max: 8, validators: { noOrphans: 'lenient' } },
  onSelectionChange: (sel) => cart.update(sel.objects, sel.holdToken),
});
```

Shorter form when the server knows the chart: omit `chart`; the adapter reads the event's pinned version.

Not yet: `selection.validators`. Without a server, `customState` (§14) works.

## 3. Options

### 3.1 Data

| Option | Type | Default | Notes |
|---|---|---|---|
| `chart` | `{ url }` or `{ bytes }` | From the state adapter | CCF location; `url` a string or `URL`, `bytes` an `ArrayBuffer` or `Uint8Array` |
| `state` | `StateAdapter` | `staticState()` | See §4 |
| `pricing` | `CategoryPricing[]` | none | Display only; prices in minor units (below) |
| `formatPrice` | `(minor, ctx) => string` | `Intl.NumberFormat` | `ctx`: `{ category, ticketType?, currency?, locale }`. One that throws is reported (`callback_failed`) and the default is used |
| `currency` | ISO 4217 code | none | For the default format: `Intl.NumberFormat(locale, { style: 'currency', currency })`, and the currency's own minor unit divides the price (`JPY` 0 decimals, `GBP` 2, `KWD` 3). Without it, the price divided by 100 with two decimals |
| `floor` | floor id | First floor | Initial floor. Not yet: only the first floor is drawn |

```ts
interface CategoryPricing {
  category: string;                                   // the category key
  price?: number;                                     // minor units
  ticketTypes?: { key: string; label?: string; price: number }[];
}
```

Prices are the host's display data; Cavea charges nothing. Each entry needs a category and a price or ticket types, every price a whole number of minor units from 0; an entry that is not (and a `pricing` that is not an array is `invalid_option`) is left out, with a development warning. The legend shows each category's price, or the range over its ticket types ("£60.00 – £95.00", message `priceRange`); the tooltip shows the hovered object's. Without `pricing` no price is shown anywhere. Prices are text: whatever `formatPrice` returns is set as text, never parsed as markup (12 §3 T7).

**Ticket types.** A selected object whose category has ticket types gets the first type by default; `ticketType` and `price` on the objects of `onSelectionChange` and `getSelection()` say which. The buyer changes it in the tooltip (§7); each change fires `onSelectionChange` again with the new type and price. `setPricing` re-prices the selection without firing it; a selected object whose type is gone gets the first again.

### 3.2 Selection

| Option | Type | Default | Notes |
|---|---|---|---|
| `selection.max` | number or `{ total, perTicketType }` | 10 | A whole number from 1 to 100 today; `{ total, perTicketType }` not yet (WEB-009) |
| `selection.min` | number | 0 | Checkout is "invalid" below this. A whole number from 0 to `max` |
| `selection.groupSize` | number | 1 | One tap selects this many adjacent seats. Not yet |
| `selection.validators.noOrphans` | `'off'`, `'lenient'`, `'strict'` | `'off'` | Not yet |
| `selection.validators.consecutive` | boolean | false | Not yet |
| `selection.categories` | string[] | all | Only these categories are selectable. Not yet |
| `selection.selectable` | `(obj) => boolean` | none | Extra host rule; called on tap only. Not yet |
| `selection.restore` | boolean | true | Restore the buyer's holds after a reload. Not yet as an option: the engine always restores the holds an adapter reports as the buyer's (`mine`, or a PRIVATE frame), and `caveaServer` always reports them (§4.1) |

### 3.3 Behaviour

| Option | Type | Default | Today |
|---|---|---|---|
| `mode` | `'picker'`, `'static'`, `'spotlight'`, `'manager'` | `'picker'` | `'picker'` only; another mode warns and runs as `'picker'` |
| `spotlight` | labels to highlight in spotlight mode | — | Not yet |
| `gestureHandling` | `'greedy'`, `'cooperative'`, `'none'` | `'greedy'` | Built |
| `zoomToSelect` | boolean | true on touch | Not yet as an option: the engine always zooms a seat too small to pick (under 18 px on touch, 5 px with a mouse) |
| `sectionDrillDown` | boolean | true when the chart has sections | Not yet |
| `initialView` | `'fit'`, section label, or bounds | `'fit'` | Built; a section on the floor shown. An unknown section is reported through `onError` (`invalid_input`) and the chart is fitted |
| `maxDpr` | number | 2 touch, 3 otherwise | Built |
| `reducedMotion` | `'auto'`, `true`, `false` | `'auto'` | Built; `'auto'` follows `prefers-reduced-motion` |

### 3.4 Interface

| Option | Type | Default | Today |
|---|---|---|---|
| `ui.zoomControls` | boolean | true | Built (§7) |
| `ui.floorSelector` | boolean | true when more than one floor | Not yet (WEB-030) |
| `ui.legend` | boolean | true | Built (§7) |
| `ui.filters` | `{ categories?, price?, accessible? }` or false | false | Not yet (WEB-030) |
| `ui.tooltip` | boolean or `(obj) => HTMLElement \| string \| null` | true | Built (§7). A function replaces the built-in content: it gets the object with its `status` (as `findObject`) each time the tooltip opens, and what it returns is placed in the positioned `tooltip` part, an element as is and a string as text; `null` or `''` shows nothing. A function that throws is reported (`callback_failed`) and nothing is shown. An element the host builds from untrusted strings with `innerHTML` is the host's risk (12 §3 T7) |
| `ui.toasts` | boolean | true | Built (§7). Off, the callbacks still fire |
| `ui.loading` | boolean | true | Built: the loading skeleton and the error state with its retry (§7, §9). Off, a chart that cannot be fetched rejects `createSeatMap` |
| `ui.holdTimer` | boolean | false | Not yet (M5): the countdown is the built-in interface's. Its data exists: `getSelection().expiresAt` is on the page's clock, so the seconds left are `(expiresAt - Date.now()) / 1000`; `onHoldExpiring` and `onHoldExpired` fire (§5) |
| `ui.listViewToggle` | boolean | true | Not yet |
| `theme` | `'light'`, `'dark'`, `'high-contrast'`, `'auto'`, or `{ base?, tokens }` | `'auto'` | Built. `'auto'` is light or dark following `prefers-color-scheme`, and its changes. `tokens` maps §8 names (with or without `--cavea-`) to values over `base` (default `'light'`; `'auto'` allowed): they are set as custom properties on the interface's root, so they win over the page's (§8). An unknown token or bad value is reported through `onError` (`invalid_input`) and the canvas keeps its theme |
| `colorblind` | boolean | false | Not yet |
| `locale` | BCP 47 tag | `navigator.language` | Built: plural forms, numbers and default prices; an invalid tag is `invalid_option`. `setLocale` changes it live |
| `messages` | Partial message catalogue | built-in | Built (below) |
| `dir` | `'ltr'`, `'rtl'`, `'auto'` | `'auto'` | Built: the interface's direction. `'auto'` is `locale`'s script's direction when the host gives a `locale` (Arabic, Hebrew, Persian, Urdu and the others written right to left), else the page's, inherited through the shadow root. The canvas is not mirrored |

**Messages.** Every string the map shows is in one English catalogue, keyed by the ids below; `messages` overrides any subset (an unknown id warns in development and is ignored; a missing one is English). Placeholders are `{name}`; numbers in them are formatted for `locale`. A message with a count is an object of plural forms, chosen with `Intl.PluralRules(locale)` and falling back to `other`: `{ one: 'Un seul siège.', other: '{count} sièges au plus.' }`. No ICU syntax and no library. A malformed message (not a string, or an object without a string `other`) is `invalid_option`.

| Id | English | Where |
|---|---|---|
| `mapLabel` | Seat map | The map's accessible name (11 §5); override it to name the venue and event |
| `mapRole` | seat map | The map's `aria-roledescription` |
| `mapHelp` | Use the arrow keys to move between seats, and Enter to choose one. | The map's description: its usage instructions |
| `place` | {index} of {count} in the row | The proxy's name: the seat's place in its row (11 §5) |
| `seatSelected`, `seatDeselected` | {seat} selected. · {seat} deselected. | Live region, `{seat}` the seat's section, row and seat |
| `selectedCount` | one: {count} seat selected. · other: {count} seats selected. | Live region, after the seat |
| `zoomIn`, `zoomOut`, `zoomFit` | Zoom in · Zoom out · Show the whole map | Zoom buttons' names |
| `legend` | Key | Legend summary |
| `available`, `selected`, `unavailable` | Available · Selected · Unavailable | Legend states and the tooltip's status |
| `loading` | Loading the seat map… | Loading state |
| `loadFailed`, `retry` | The seat map could not be loaded. · Try again | Error state |
| `offline` | You are offline. The seat map will load when you are back online. | Error state, while offline before the first frame (§9) |
| `contextLost` | The seat map cannot be drawn right now. | Error state, after the WebGL context has been lost 5 s (§9) |
| `soldOut` | Sold out | Drawn on sold-out sections by the engine (`SetMessage`) |
| `row`, `seat` | Row {row} · Seat {seat} | Tooltip, proxy name, live region |
| `priceRange` | {min} – {max} | Legend and tooltip, over ticket types |
| `accessible`, `companion`, `restrictedView`, `aisle` | Wheelchair accessible · Companion seat · Restricted view · Aisle seat | Tooltip: the seat's flags in words |
| `ticketType` | Ticket type | Name of the tooltip's ticket-type choice |
| `taken` | {seat} was just taken by someone else. | Toast: `onObjectUnavailable` (`{seat}` the label) |
| `notFree` | That seat is not available. | Toast: refusal `not_free` |
| `max` | one: You can choose only one seat. · other: You can choose up to {count} seats. | Toast: refusal `max` |
| `holdLimit` | You cannot hold any more seats. | Toast: refusal `hold_limit_exceeded` |
| `holdFailed` | That seat could not be held. Please try again. | Toast: any other refusal |
| `holdLost` | one: Your hold on one seat has ended. · other: Your hold on {count} seats has ended. | Toast: `onHoldLost` |
| `holdExpired` | Your hold expired and your seats were released. | Toast: `onHoldLost` with `onHoldExpired` |
| `reconnecting` | Live seat updates were interrupted. Reconnecting… | Toast: `state_disconnected` |
| `dismiss` | Dismiss | A toast's close button |

### 3.5 Loading

| Option | Type | Default | Notes |
|---|---|---|---|
| `assets.baseUrl` | string or `URL` | Package location | Where `cavea_web_bg.wasm` and `glyphs.png` are served from; relative to the page if relative |
| `assets.wasm` | `Response`, `ArrayBuffer`, `Uint8Array`, `WebAssembly.Module`, or URL | — | Bring your own fetch. The WASM is loaded once per page: the first map's source is used by every later map |
| `assets.image` | `(hash) => string \| URL \| Blob \| Promise<…>` | The adapter's `image`, if any | Where the chart's background images are (03 §2.7: images with role `background`), by content hash (32 lowercase hex digits, checked before the call): a URL the map fetches, or the bytes. Called after the first frame (below) |
| `fallback` | `'auto'`, `'canvas'`, `'list'` | `'auto'` | Force a step of the capability ladder. Not yet (REN-071) |
| `signal` | `AbortSignal` | none | Cancels the map (§9): aborted before `createSeatMap` settles, it rejects with `aborted` and removes everything; after, it destroys the map |
| `worker` | boolean | false | P2 (WEB-040) |

### 3.6 Validation and development builds

`createSeatMap` validates the options object before it fetches anything. An invalid value rejects the promise with a fatal `CaveaError` whose `code` is `invalid_option` and whose `option` names it (`selection.max`, `theme`, …). A `state` adapter must have `connect` and `disconnect`, and `hold` and `release` together or neither. Callbacks must be functions.

In a **development build** the package warns on the console, once per map and message, about unknown keys (`unknown option "colour" is ignored`), options and callbacks that are not built yet (`option "pricing" is not supported yet and is ignored`), methods that are not built yet (`setPricing() is not supported yet`), calls on a destroyed map, and errors when no `onError` is given. A development build is any build in which `process.env.NODE_ENV` is not `"production"`: bundlers replace that expression with a literal in production builds, and on a page with no bundler and no `process` it counts as development. In production these warnings are silent; the ignored options are ignored all the same. Warnings go through one function and are the only console output of the package.


## 4. State adapters

The renderer does not care where seat state comes from. An adapter supplies it.

```ts
interface StateAdapter {
  /** The chart, when the options give none (the event's pinned version, for example). */
  chart?(signal: AbortSignal): Promise<ChartSource>;
  /** Where the background image with this content hash is, unless `assets.image` says (§3.5). */
  image?(hash: string): string | URL | Blob | Promise<string | URL | Blob>;
  /** Called once, at start, with the WASM, chart and atlas fetches. Resolve with the first state. */
  connect(ctx: AdapterContext): Promise<Snapshot>;
  /** Hold-on-select. Without `hold` and `release` the map runs in manual session mode. */
  hold?(labels: string[]): Promise<HoldResult>;
  release?(labels: string[]): Promise<void>;
  /** The engine saw a gap in the deltas: send a new snapshot through ctx. */
  resync?(): void;
  /** The page was hidden, and shown again (WEB-025 drops and resumes the realtime connection). */
  pause?(): void;
  resume?(): void;
  disconnect(): void;
}

type ChartSource = { url: string | URL } | { bytes: ArrayBuffer | Uint8Array };

type Snapshot =
  | { kind: 'binary'; bytes: ArrayBuffer | Uint8Array }                       // one SNAPSHOT frame (08 §8)
  | { kind: 'map'; unavailable: string[]; mine?: string[]; expiresAt?: number }; // labels

type HoldResult =
  | { ok: true }
  | { ok: false; code?: string; failed?: { label: string; reason: string }[] };

interface AdapterContext {
  readonly signal: AbortSignal;                       // aborted on destroy or a failed load
  applyFrame(bytes: ArrayBuffer | Uint8Array): void;  // SNAPSHOT, DELTA or PRIVATE frame, as received
  applySnapshot(snapshot: Snapshot): void;            // replaces all state
  applyDelta(d: { unavailable?: string[]; available?: string[] }): void;  // labels
  setMine(labels: string[], expiresAt?: number): void; // the buyer's own holds, by label
  setHoldToken(token: string | null): void;           // reported in getSelection() and onSelectionChange
  setClockOffset(ms: number): void;                   // the server's clock minus Date.now()
  reportError(error: unknown): void;                  // to onError, not fatal
}
```

How it works:

- **Two forms of state.** An adapter speaks either the server's wire format (`binary` snapshots and `applyFrame`, handed to the engine byte for byte; HELLO, RESYNC, PING and CLOSE frames are the adapter's own business) or labels (`map` snapshots, `applyDelta`, `setMine`). Label state is converted to the engine's per-object codes inside the WASM in one call per update (`View.setSnapshotLabels`, `applyDeltaLabels`, `setPrivateLabels`), never one call per seat. Label deltas are numbered by the SDK, so they never trip a resync; an adapter does not mix the two forms. Unknown labels are skipped and reported through `onError` (`state_failed`).
- **`mine` and `expiresAt`.** Labels the buyer's hold token holds; the engine keeps them selected, selects them after a reload, and reports `expiresAt` in `onSelectionChange`. A `map` snapshot without `mine` leaves the buyer's set as it was. A held seat that leaves the set leaves the selection (`onHoldLost`); an adapter drops the buyer's seats at their expiry this way, and that is when `onHoldExpired` fires (§5).
- **Clocks.** Expiries an adapter reports (`expiresAt`, PRIVATE frames) are on the server's clock. `setClockOffset` tells the map the server's clock minus the page's `Date.now()`; the map then reports `expiresAt` on the page's clock and times `onHoldExpiring` by the server's. Without it the two clocks are taken to agree.
- **Order.** Calls on the context made before the first snapshot is applied (before the chart is loaded, or before `connect` resolves) are kept and applied after it, in order. A snapshot that arrives before the chart is applied once the chart is decoded.
- **Holds.** With `hold` and `release`, each engine `HoldRequest` and `ReleaseRequest` (04 §16) is passed to the adapter. `hold` and `release` are called one at a time, in the order the engine emits them, each after the previous call's promise has settled; the engine already coalesces taps into at most one hold in flight. `{ ok: true }` holds every label of the request. A refusal is all or nothing (02 §4.3): `failed` names the objects at fault with their reasons (the server's per-object `reason`, such as `object_not_free`), and `code` the request's (such as `hold_limit_exceeded`, `rate_limited`, or `not_found` for a hold token that is gone, which drops the whole selection); objects `failed` does not name are requested again with the next hold. A rejected or thrown `hold`, or one without an answer in 20 seconds, is reported as `hold_failed` through `onError` and fails the request with code `hold_failed`. A failing `release` is reported as `release_failed`. A result after `destroy` is ignored. Retrying a request is the adapter's choice (`caveaServer` retries once, §4.1).
- **Manual session.** An adapter without `hold` and `release` puts the engine in `manual` session mode: taps select at once and nothing is held (D-14).
- **Lifetime.** `chart` and `connect` are called once, at start. `disconnect` is called once, last, on `destroy` or when loading fails; no other method is called after it, and the context's calls do nothing.
- **Errors.** A rejected `connect`, or a first state the engine refuses (a malformed frame, a snapshot of the wrong length), rejects `createSeatMap` (`state_connect_failed`): a picker without state would offer seats it cannot know are free. Unknown labels in it are only reported. Every later adapter error goes to `onError` and never reaches the page as an exception or an unhandled rejection.

Built-in adapters:

| Adapter | Use |
|---|---|
| `caveaServer(options)` | Talks to `cavea-server`: binary snapshot over HTTP, deltas over WebSocket with SSE and polling fallback, holds over HTTPS (§4.1) |
| `staticState({ unavailable })` | Fixed state by label, no holds, for static and spotlight modes or demos |
| `customState(impl)` | The host's own backend. Implement `connect`, and `hold` and `release` if you want hold-on-select |

### 4.1 `caveaServer`

```ts
state: caveaServer({ baseUrl: 'https://tickets.example.com/cavea', publishableKey: 'pk_live_…', event: 'hamlet-2027-03-14' })
```

| Option | Type | Default | Notes |
|---|---|---|---|
| `baseUrl` | string | — | Where `cavea-server` answers; relative to the page if relative |
| `publishableKey` | string | — | |
| `event` | string | — | Event key |
| `session` | `'start'`, `'continue'`, `'manual'`, `'none'` | `'start'` | Below |
| `holdToken` | string | — | Required for `continue`, refused otherwise |
| `sessionToken` | string or `() => Promise<string>` | — | Signed by the customer's backend (`08-server.md` §11). Sent as `Cavea-Session` on hold-token creation, hold and release, and in SUBSCRIBE once known. A function is called at the first hold, and once more (with the request repeated) when the server answers `401` with `details.session` `expired` |
| `transport` | `'auto'`, `'ws'`, `'sse'`, `'poll'` | `'auto'` | Below |
| `pauseWhenHidden` | boolean | true | Below |

An invalid option throws a fatal `CaveaError` (`invalid_option`, `option` such as `state.event`) from `caveaServer` itself.

**Start.** `chart` (when the host gives no chart) and `connect` start together, alongside the WASM: the event info (`GET /v1/public/events/{event}`, key in `Authorization`), the snapshot (`GET …/state?key=`, a simple request) and, if there is a hold token to restore, `GET /v1/public/hold-tokens/current`. The chart is the event info's `chart.url` under `baseUrl`, and a background image is `GET /v1/public/assets/{hash}` under `baseUrl` (no credential, immutable, 08 §6.2). `connect` resolves with the binary snapshot; a failed info or snapshot request rejects it, so `createSeatMap` rejects with `state_connect_failed`. So does an event whose info says `require_session` when `session` is `start` or `continue` and no `sessionToken` is given. The realtime connection opens after the snapshot, from its `seq`.

**Transports.** SNAPSHOT, DELTA and PRIVATE frames go to the engine byte for byte; the adapter reads only a frame's type and `seq`, HELLO, PING and CLOSE. A DELTA advances the adapter's `seq` only if it starts at it; the engine refuses one that does not and asks for a resync.

| Transport | How | The buyer's own holds |
|---|---|---|
| WebSocket | `GET /v1/public/rt?key=`; SUBSCRIBE (version 1, event, `seq`, the hold token and session token if any); every PING answered with PONG; 60 s without any frame (the server pings every 25 s) counts as a dropped connection | PRIVATE frames, when SUBSCRIBE carried the hold token. A token created after the connection opened is followed by reconnecting with it at once |
| SSE | `GET …/stream?key=&last_seq=`; each event's `data` is a base64 frame. The adapter closes the `EventSource` on every error and reconnects itself, from its `seq`, with the backoff below, rather than leaving dropped connections to `EventSource`'s fixed delay | No PRIVATE frames (a hold token never goes in a URL): read from `hold-tokens/current` (`setMine`) after each hold and release, at every (re)connection, at the expiry, every 60 s while any hold is live, and 5 s after a read that failed. A read that a later hold or release overtakes is dropped |
| Polling | `GET …/state?key=&after_seq=` every 2 s: `200` with a DELTA or SNAPSHOT, `204` when current | As SSE |

- **Auto.** WebSocket first. A transport that fails twice in a row without ever delivering a HELLO on this page (a poll answer counts as one) is given up for the next: SSE, then polling. One that has delivered a HELLO is kept for the page's life and reconnected however often it drops (a server restart is not a reason to fall back), and a fallback that works stays: the better transport is not tried again until the page reloads. A forced transport is the only one tried. A transport the browser lacks is skipped.
- **Reconnection.** After a dropped connection, a failed one, CLOSE 3 or 6 or an unknown reason: `min(30 s, 500 ms × 2^(n−1))` for the n-th consecutive failure, times a random factor from 0.5 to 1, and never sooner than a polling `429`'s `Retry-After`. A HELLO resets the count. The connection resumes from the adapter's `seq` (the server sends the DELTAs since, or a SNAPSHOT). The first drop of a live connection is reported (`state_disconnected`, recoverable); the map stays usable meanwhile and holds still go over HTTPS.
- **CLOSE.** 1 (archived), 2 (chart upgraded: reloading the chart in place is a later task), 4 (protocol version) and 5 (unknown event for this key) stop realtime for the page, reported as `state_disconnected`. CLOSE 5 after a SUBSCRIBE that carried the hold token means the token may not be followed: its session has ended (as below) and the adapter subscribes again at once without it.
- **HELLO** sets the clock offset (its server time minus `Date.now()`). A chart hash other than the event info's means the event moved to another chart version since the page loaded: reported (`state_failed`, "reload the page"); state keeps flowing to the loaded chart until then.
- **Resync.** When the engine reports a gap, or the host calls `map.resync()`, the adapter reconnects the stream with `seq` 0 (the next poll asks from 0), so the SNAPSHOT arrives on the stream itself, in order with the deltas after it. A snapshot fetched over HTTP could be older than the stream's position and never line up.
- **Pause.** With `pauseWhenHidden`, a page hidden for 10 s drops its connection (the poll loop stops); shown again, it reconnects at once from its `seq`, so the server sends the deltas it missed if its 60 s buffer still has them, else a SNAPSHOT. Holds are not released by hiding.
- **Malformed frames** (a text message, bad base64, a HELLO or `seq` that does not decode) are reported (`state_failed`) and the connection is dropped and resumed.

**Sessions.**

| `session` | Hold token | Selecting |
|---|---|---|
| `start` | Created (`POST /v1/public/hold-tokens`) at the buyer's first tap, not on load: an eager token would cost a request (from the holds budget, `08-server.md` §12), a hold record on the server and, with `require_session`, one of the buyer's `max_tokens_per_sub`, for every visitor who never taps. The first tap waits one more round trip | Hold on select |
| `continue` | `holdToken`, created by the host's backend; checked with `hold-tokens/current` at start and never stored | Hold on select. Once the token has ended, taps fail with `session_ended` and `state_failed` is reported once |
| `manual` | None | Taps select without holding; the host holds at checkout from `getSelection()` |
| `none` | None | As `manual` today: hiding the selection needs `mode: 'static'` (§12), which is not built |

**Holds.** `hold` posts `{hold_token, objects: labels}` to `…/events/{event}/hold`; `release` posts to `…/release`. A token ends when the server answers `404` with `details.missing` `hold_token`, or `hold-tokens/current` answers `404`, or CLOSE 5 follows a SUBSCRIBE with it: the adapter forgets it (`setHoldToken(null)`, removed from storage) and sets the buyer's holds to none. In `start` mode the next tap creates a new token.

| Server answer to a hold | `HoldResult` | Reported |
|---|---|---|
| `200` | `{ ok: true }` | — |
| `409 object_not_free` and other refusals naming objects | `{ ok: false, code, failed }`, `failed` the objects' `label` and `reason` (`not_free`: the seat leaves with `onObjectUnavailable`) | — |
| `422 hold_limit_exceeded` | `{ ok: false, code }` (`onSelectionInvalid` with that rule) | — |
| `404`, `missing: hold_token` | `{ ok: false, code: 'not_found' }`: the engine drops the selection (`onHoldLost`) | — |
| `404` for objects | `code: 'unknown_object'` | `hold_failed` |
| `401` `expired`, with a `sessionToken` function | Called again; the request repeated once | — |
| `429`, `503` | Repeated once after `Retry-After` if that is at most 2 s; otherwise, or failing again, `{ ok: false, code }` (`rate_limited`, `overloaded`, `unavailable`) | `hold_failed` |
| A network failure | Repeated once after 500 ms; failing again, `hold` rejects: the map fails the request with `hold_failed` | `hold_failed` |
| Any other status | `{ ok: false, code }` with the problem's `code` | `hold_failed` |

Each request is cancelled after 8 s, its body included. A hold gives up 18 s after it started, token creation, a session-token refresh and retries included, so the map's own 20 s limit is never reached and holds stay one at a time; a retry that would end past that is not made. Hold-token creation follows the same table. A release is repeated the same way; a release whose token has ended succeeds; any other refusal rejects (`release_failed`).

**Persistence.** In `start` mode the token is kept in `sessionStorage` under `cavea:hold:<baseUrl>|<publishableKey>|<event>` (the base URL resolved and without a trailing slash), so it is per tab, per server, per workspace (the key names it) and per event. On load a stored token is checked with `hold-tokens/current`: known, its holds come back selected and held with one `onSelectionChange`, which carries the token; unknown (expired), it is removed silently and the session starts afresh at the next tap. Where `sessionStorage` cannot be used (private modes, sandboxed frames, a full quota) the token is kept in memory for the page's life. `getSelection().holdToken` and `onSelectionChange` give it to the host for booking.

**Clock.** The buyer's clock can be minutes off. The adapter sets the offset from each HELLO; before any HELLO, from hold-token creation (the token's `expires_at`, plus half a second for its truncation, less the event's `hold_ttl_seconds`, against the middle of the request); on polling, from a readable `Date` header (a same-origin server only: `Date` is not exposed across origins). Without any of these (polling across origins with a restored token) the page's clock is trusted.

**Measured locally** (2026-11-11; AMD Ryzen 9 7900X, Linux; `cavea-server` release build and PostgreSQL 18 on the same machine over loopback; two headless Chromium 157 instances with software WebGL2 (SwiftShader) on `apps/demo` in server mode, one driven by touch, one by mouse, over the DevTools protocol). Hold to visible, from browser A's hold response arriving (its `fetch` resolving) to browser B's engine reporting the seat unavailable (`findObject`, sampled every millisecond in B), both on the machine's one clock, over 34 taps on the WebSocket: median 1 ms, 95th percentile 3 ms, maximum 4 ms; release to visible the same. The taps were about a second apart, so each delta was the first publication after a quiet interval and went out at once (D-27); a change within 100 ms of the previous publication waits for the interval (one such tap: 98 ms). Over SSE 2 ms; over polling up to the 2 s interval (561 ms seen). This is a loopback measurement, not a network or device result; the 250 ms criterion (`ROADMAP.md` M2, `01-architecture.md` §5) held with that margin here.

## 5. Callbacks

| Callback | Fires when | Payload | Today |
|---|---|---|---|
| `onReady` | Chart-ready | `{ chart: { name, seats, objects }, backend: 'webgl2', marks }`: timing marks in ms since `createSeatMap` (04 §24: `init_start`, `wasm_ready`, `gpu_ready`, `chart_fetched`, `chart_decoded`, `first_frame`, `state_ready`; `interactive` comes after, in `onMetrics`; also `performance.mark('cavea:<name>')`) | Built; fires just before the promise resolves. `first_frame` is missing while the container is hidden |
| `onSelectionChange` | The selection changes, or the buyer picks another ticket type (§3.1) | `{ objects, holdToken, valid, expiresAt }` | Built; `valid`: at least `selection.min` objects; `expiresAt` in Unix ms on the page's clock (`Date.now()`), corrected for the server's (§4) |
| `onObjectSelect` / `onObjectDeselect` | One object is added or removed | Object | Built |
| `onSelectionInvalid` / `onSelectionValid` | Rule state changes | `(rule, objects)` / — | Built: rule `min` (objects: the selection) or a refusal (`max`, `not_free`, a hold code: objects refused) |
| `onHoldPending` / `onHoldSettled` | Hold requests start and finish | — (use to disable the checkout button) | Built: pending when the first of a run of holds starts, settled when the last is answered |
| `onHoldFailed` | A hold was refused | `[{ object, reason }]` | Built |
| `onHoldLost` | The buyer's hold on selected objects is gone (expired, released elsewhere, token gone) | Objects; they also leave the selection | Built (new) |
| `onObjectUnavailable` | A selected object was taken | Object | Built |
| `onHoldExpiring` | 60 s before expiry | Seconds left | Built: once per expiry the adapter reports, timed by the server's clock (§4); at once with the seconds left if the expiry is nearer than 60 s when it becomes known (after a reload, say). One timer, recomputed when it fires and when the page is shown again, since hidden pages run timers late; none after `destroy` |
| `onHoldExpired` | The hold token expired | — | Built: when the buyer's held seats leave the selection (after `onHoldLost`) within 5 s of their expiry or after it, by the server's clock. A hold given up earlier (released elsewhere, deselected) fires no expiry |
| `onObjectHover` / `onObjectFocus` | Pointer or keyboard focus | `(object or null, at)`: `at` the object's centre in CSS pixels | Built. Focus is the keyboard's (11 §3.1): fired when the focus shown moves or appears, with `null` when it is hidden (Escape, the map losing focus, a tap), and again when its seat's state changes or the camera settles after moving it on screen |
| `onSectionEnter` | The camera drills into a section | Section | Not yet |
| `onFloorChange` | The floor changes | Floor | Not yet |
| `onCameraChange` | Pan or zoom settles | `{ centre, zoom, level }` | Built |
| `onMetrics` | At most once a second, only after frames | `{ frames, frameMs: { median, p95, max }, drawCalls, visibleSeats, level, backend, pixelRatio, marks }` (04 §24): frame times over the last 120 frames, the rest for the last frame drawn | Built (REN-044). Without it frames are not timed |
| `onError` | Any recoverable or fatal error | `CaveaError` | Built |

**Where the types come from** (FND-007, D-71). The JSON the WASM hands the SDK (the engine's events, `View.object`'s payload) and the option names it takes (themes, gesture handling, levels of detail) are typed by `packages/renderer/src/generated/engine.ts`, which `cargo xtask types` writes from Rust definitions in `crates/cavea-web/src/schema.rs`; the SDK's own types (`ThemeName`, `GestureHandling`, `Metrics.level`) are those. `cargo test -p cavea-web` ties the definitions to the engine (an exhaustive conversion from each protocol type, so a field or variant added there does not compile until it is added) and to the JSON the WASM writes by hand (every event read back with unknown fields refused); `cargo xtask types --check` fails while the TypeScript is stale. The WASM carries no serde. The options object, the callbacks and their payloads are the SDK's own TypeScript, validated in `options.ts`: they do not cross into the WASM as JSON.

A callback that throws is caught and reported through `onError` as `callback_failed`; if `onError` itself throws, that is swallowed. No callback fires after `destroy`. Callbacks fire in the engine's order: events caused by a method called from inside a callback are delivered after the events already being delivered. Selection callbacks can fire before `createSeatMap` resolves, if the buyer taps before the first state arrives.

The object payload:

```ts
interface SeatObject {
  label: string;            // "Orchestra-C-12"
  kind: 'seat' | 'table' | 'booth' | 'area';
  section?: string; row?: string; seat?: string; floor: string;
  category: { key: string; label: string };
  ticketType?: string;      // chosen by the buyer in the tooltip, if types exist
  quantity?: number;        // areas
  flags: { accessible: boolean; companion: boolean; restrictedView: boolean; aisle: boolean };
  price?: number;           // from pricing, minor units
}
```

Today `kind` is always `'seat'` (tables, booths and areas are not compiled yet), `floor` is the floor name of the object's section (the first floor for a seat outside sections), and `quantity` is not in the type until areas exist. `section`, `row` and `seat` are left out when the chart has none. `price` is the chosen ticket type's, else the category's, and is left out without `pricing`; `ticketType` is set on selected objects whose category has ticket types (§3.1).

## 6. Methods

| Method | Notes | Today |
|---|---|---|
| `map.getSelection()` | Current objects and hold token | Built: `{ objects, holdToken, valid, expiresAt }` as `onSelectionChange` |
| `map.select(labels)`, `map.deselect(labels)`, `map.clearSelection()` | Subject to the same rules as taps | Built; all or nothing; unknown labels are reported (`invalid_input`) |
| `map.selectBestAvailable({ quantity, categories?, accessible? })` | Uses the adapter's `bestAvailable` | Not yet |
| `map.setPricing(p)`, `map.setFilter(f)`, `map.setTheme(t)`, `map.setLocale(l)` | Live updates | `setTheme` (§3.4 values), `setPricing` and `setLocale` built: the interface is rendered again, and `setLocale` also gives the engine the `soldOut` message; an invalid value is `invalid_input`. `setFilter` not yet (WEB-030) |
| `map.refreshTheme()` | Reads the `--cavea-*` custom properties again (§8) | Built: for a change the map cannot see, such as a stylesheet swapped in; does nothing when nothing changed |
| `map.setMode(m)`, `map.setSpotlight(labels)` | | Not yet |
| `map.zoomTo(target, opts?)` | Section label, object label, floor, bounds, or `'fit'` | Built for `'fit'`, bounds, a section label and a seat label on the floor shown (a string names a section if one has that label, else a seat); floors not yet. Animated unless `{ animate: false }`. The camera stays within the chart, so a seat near its edge may not be centred |
| `map.zoomIn()`, `map.zoomOut()` | | Built: twice or half the zoom about the centre |
| `map.setFloor(id)` | | Not yet |
| `map.findObject(label)` | Details and current status | Built: the `SeatObject` and `status` (`'available'`, `'unavailable'` or `'selected'`), or `null` for an unknown label |
| `map.listSections()`, `map.listCategories()` | With availability counts | Built. Sections: `{ label, floor, seats, available }`, `available` counting seats not drawn unavailable (free, or selected by this buyer). Categories: `{ key, label, color, objects }` (`color` `#rrggbbaa`, `objects` bookable objects); per-category availability not yet (the engine keeps no per-category counters) |
| `map.showListView(bool)` | Switch between map and accessible list | Not yet |
| `map.resync()` | Force a fresh snapshot | Built: calls the adapter's `resync` |
| `map.toImage({ width, height })` | PNG blob of the current view (thumbnails, confirmation pages) | Not yet |
| `map.destroy()` | Releases the GL context, sockets and listeners. Does not release holds | Built: disconnects the adapter, aborts its `signal`, removes every listener, observer and pending frame and timer, frees the WASM view and removes the canvas. Later calls do nothing (a development warning); a second `destroy` does nothing |

Methods never throw. Methods that are not built exist and only warn in development; they are not in the TypeScript type.

`createSeatMap` returns a promise that resolves at chart-ready (the chart decoded and the first state applied; it does not wait for a frame, so a hidden container does not hold it up) and rejects only if no step of the capability ladder works. The ladder (Canvas 2D, then the list view) is not built yet (REN-070, REN-071): today WebAssembly and WebGL2 are required, and without them the promise rejects with `wasm_unavailable` or `webgl_unavailable`. The ladder hooks in where `createSeatMap` creates the WebGL2 view. It also rejects for invalid options, outside a browser (`not_browser`), when the chart or the first state cannot be had (§9), and when the host aborts `signal` (`aborted`): the way to cancel a map whose promise is still pending, for example while the error state waits for a retry.

## 7. Built-in interface

All built-in controls live in a shadow root on the container, so host styles cannot break them and theirs cannot leak. The shadow root holds one root element (`.r`) with the canvas, which fills the container and is the map's one tab stop (`role="application"`, named by `mapLabel`, with the keyboard focus's proxy in its fallback content, 11 §5), the controls over it, and the selections' visually hidden live region. The container's own children are not displayed while the map is in it. The container is `display: block` unless the page styles it otherwise; the `hidden` attribute still hides it.

| Element | Option | Today |
|---|---|---|
| Zoom controls | `ui.zoomControls` | Built: in, out and fit, a column of 44 px buttons in the top inline-end corner. In and out are marked `aria-disabled` at the zoom limits the engine reports with each camera change (04 §3), so focus stays on them; fit flies to the chart |
| Floor selector | `ui.floorSelector` | Not yet (WEB-030). Segmented control; a menu above five floors |
| Legend | `ui.legend` | Built: a disclosure (`<details>`) in the bottom inline-start corner, open when the map is at least 600 px wide when created, else closed. Each category with bookable objects, with its colour (`--cavea-category-<key>` over the chart's), name and price (§3.1); then the states with the shapes the map draws them in: available solid, selected with a tick, unavailable hollow and crossed (11 §6) |
| Filters | `ui.filters` | Not yet (WEB-030). Category chips, price range, "accessible only" |
| Tooltip | `ui.tooltip` | Built, below |
| Area sheet | | Not yet (with areas). Quantity stepper for general-admission areas |
| Toasts | `ui.toasts` | Built, below |
| Hold timer | `ui.holdTimer` | Not yet (M5). Optional countdown with an "I need more time" action that calls the host |
| List-view toggle | `ui.listViewToggle` | Not yet (A11Y-014). Switches to the accessible list |
| Loading and error states | `ui.loading` | Built: a skeleton (a block 24 px inside the map, pulsing unless motion is reduced, with the `loading` message as a polite status) until the first frame with the chart is drawn; the chart's bounds are unknown until it arrives, so the skeleton is the map's own box. The error state (§9) replaces it with the `loadFailed` message and a Retry button (`role="alert"`). While either shows, the zoom controls and legend are hidden |

Each element is on unless its option is `false`; one that is off is never created, costs no listener, and its callbacks still fire. It stays in the bundle: the interface is one module, and dropping an element at build time would need a separate entry (a packaging question for WEB-013 and WEB-018).

**Tooltip.** For the hovered object (04 §14: a mouse or pen over a seat), or the keyboard's focused one (11 §3.1): the section, row and seat, the category with its colour and price (the range over ticket types until one is chosen), the flags in words (`accessible`, `companion`, `restrictedView`, `aisle` messages) and the status (`available`, `unavailable` or `selected`). It is centred 14 px above the object, or below it when there is no room above, and kept 8 px inside the map. It takes no pointer events, so it never steals hover. Escape (listened for only while it shows), a pan or zoom, and the pointer leaving the object dismiss it; dismissed with Escape, it stays hidden until the hover moves to another object.
- **Touch.** There is no hover. A long press shows the tooltip for the object under it without selecting (04 §14), and the next tap clears it. A tap that selects one object pins the tooltip to it, anchored at the tap, until the next press on the map, a pan or zoom, or Escape: on touch always, so the price and flags reach touch buyers, and with a mouse when the object's category has several ticket types (the hover tooltip shows the rest).
- **Ticket types.** A pinned tooltip on a selected object whose category has several ticket types shows them as buttons (`aria-pressed` on the chosen one) in a group named by the `ticketType` message, each with its price. Choosing one reports it (§3.1). A pinned tooltip ignores hover elsewhere and takes pointer events; it does not take focus.
- **Replaced by the host.** With a function for `ui.tooltip` (§3.4), the host's element or text is shown in the same positioned box (the `tooltip` part, which the host styles with `::part(tooltip)`) and the built-in content is not rendered; the ticket-type choice is then the host's (the default type of §3.1 still applies). With `ui.tooltip: false`, nothing is shown and `onObjectHover` gives the host the object and its centre.

**Toasts.** In words, from the catalogue: a selected seat taken (`onObjectUnavailable`, message `taken`), the refusals of 04 §16 (`max`, `not_free`, `hold_limit_exceeded`, and any other hold code as `holdFailed`; `min` is readiness, not a refusal, and shows nothing), a hold lost or expired (`holdLost`, `holdExpired`) and a dropped realtime connection (`reconnecting`). They stack at the top of the map, newest last, at most three (the oldest goes first); a message already shown is replaced, not repeated. Each closes after 5 s, or with its Dismiss button; while the pointer is over them or focus is in them none closes. They sit in one `role="status"` region (`aria-live="polite"`), which also speaks for refusals, losses and expiry until the assertive region of 11 §5 exists (A11Y-013); the buyer's selections are said in a region of their own (11 §5), which leaves out the removals a toast explains. They slide in unless motion is reduced (`reducedMotion`, or `prefers-reduced-motion`). Each toast has one timer, which ends with it; an idle map has none.

**Layout.** Controls are absolutely placed over the canvas and never resize it; they use logical properties, so a right-to-left page (or `dir`) mirrors them. Buttons are at least 44 × 44 CSS px. Nothing in the interface makes the canvas draw: hover changes the tooltip in the DOM, and the canvas draws only for its own reasons (the hover ring). Text is `max(14px, .875rem)`, so it follows the buyer's text size; the font is the page's (inherited through the shadow root) unless `--cavea-font` is set.

**Parts** for `::part()` styling: `zoom-controls`, `zoom-in`, `zoom-out`, `zoom-fit`, `legend`, `legend-summary`, `legend-categories`, `legend-states`, `tooltip`, `ticket-types`, `ticket-type`, `toasts`, `toast`, `toast-dismiss`, `loading`, `error`, `retry`; every button also has `button`. Inside the shadow root, `.r` carries `data-s` (the scheme shown), `dir` and `lang`.

## 8. Theme tokens

Set as CSS custom properties on the container (or any ancestor) or passed as `theme` (§3.4).

| Token | Meaning |
|---|---|
| `--cavea-bg` | Canvas background, and the controls' surface |
| `--cavea-fg`, `--cavea-fg-muted` | Text; muted text and control borders |
| `--cavea-seat-available-stroke` | Stroke of available seats (fill is the category colour) |
| `--cavea-seat-unavailable-fill`, `--cavea-seat-unavailable-stroke` | Also the legend's unavailable shape and the loading skeleton |
| `--cavea-seat-selected-fill`, `--cavea-seat-selected-ink` | Also the legend's selected shape |
| `--cavea-ring-hover`, `--cavea-ring-focus` | The focus ring is also the controls' focus outline |
| `--cavea-section-tint` | Strength of the overview tint, 0–1 |
| `--cavea-dim` | Opacity of filtered-out objects |
| `--cavea-font` | Font family for dynamic text (section labels, text objects, labels the baked atlas cannot draw) and controls. Default for the canvas: the system stack `system-ui, -apple-system, "Segoe UI", Roboto, "Noto Sans", Arial, sans-serif`; for the controls: the page's font. Only fonts the page already has are used; the map never fetches one |
| `--cavea-radius`, `--cavea-shadow` | Controls (default `8px` and a soft shadow) |
| `--cavea-category-<key>` | Override one category's colour, on the canvas and in the legend |

**How tokens flow.** The interface's stylesheet reads each token with `var()` through the shadow boundary, over the built-in scheme's defaults (the engine's, 04 §19), so a change restyles the controls at once with no script. The canvas gets the tokens through `View.setTheme`: the SDK reads them from the computed style of the shadow root's `.r` (where `theme.tokens` are set, so they win over the page's), turns colours in any CSS syntax (`rgb()`, `oklch()`, names) into `#rrggbbaa` by painting one pixel on a 1 × 1 canvas, and sends them only when they differ from what the view has; the canvas redraws on the next frame without reloading the chart.

**Noticing a change.** CSS has no event for a custom property changing, so the SDK reads them again when: the theme is set (`setTheme`), the chart's categories become known, `prefers-color-scheme` changes (tokens in a media query), an attribute of the container, `<html>` or `<body>` changes (a `MutationObserver` on attributes only: class, style and `data-*` toggles, the common ways sites switch themes), and when the host calls `map.refreshTheme()`. A change by any other path (a stylesheet swapped in, a rule on an ancestor's class) needs `refreshTheme()`. Nothing polls and nothing runs per frame: with no change, a read costs one style lookup and sends nothing.

**Contrast.** The built-in schemes meet WCAG 2.2 AA for the interface, checked by calculation in the SDK's tests: text and muted text on the surface and on the skeleton at least 4.5:1, control borders, the focus ring, and the legend's state shapes at least 3:1 against the surface. The focus outline is drawn over a 4 px ring of `--cavea-bg`, so it keeps its contrast over any part of the chart. Under `forced-colors`, the browser's system colours apply to the controls; the legend's swatches keep their colours (`forced-color-adjust: none`). A canvas theme built from system colours (11 §6) is not built yet.

## 9. Errors and degradation

```ts
class CaveaError extends Error {
  code: CaveaErrorCode;  // stable, below
  fatal: boolean;        // fatal errors reject createSeatMap; after it resolves every error is recoverable
  option?: string;       // for invalid_option
  cause?: unknown;
}
```

Codes: `invalid_option`, `not_browser`, `wasm_unavailable`, `webgl_unavailable`, `chart_fetch_failed`, `bad_chart`, `state_connect_failed` and `aborted` (fatal, at start); `state_failed` (an adapter error, refused or unknown state), `state_disconnected` (`caveaServer`'s realtime connection dropped and is reconnecting, or stopped for good, §4.1), `hold_failed`, `release_failed`, `callback_failed`, `glyph_atlas_failed` (seats are drawn without labels), `image_failed` (a background image could not be fetched, decoded or used: the map is drawn without it), `context_lost` (the WebGL context was lost and not restored within 5 s; the map redraws if it comes back), `invalid_input` (a method's input refused) and `engine_failed` (a frame or input call failed). `chart_fetch_failed`, `wasm_unavailable` and `state_connect_failed` are also reported as recoverable while the error state offers a retry (below).

**Retries** (D-53). The WASM, the chart (the adapter's `chart()` and the fetch), the adapter's first state (`connect`) and the glyph atlas are each tried again after a failure that may succeed later: three retries, after 1, 2 and 4 s (doubling, capped at 30 s), each times 0.5 to 1.5 at random so that maps on many pages do not retry together. A wait does not end while the page is hidden or the browser is offline: the next try waits for the page to be shown and online. Never tried again, because they cannot succeed: an HTTP status from 400 to 499 other than 408 and 429 (a chart the server no longer has, an unknown event, a refused key), WebAssembly that does not compile or link or that a Content Security Policy refuses (`CompileError`, `LinkError`, `EvalError`), a chart that does not decode (`bad_chart`), and a browser without WebAssembly or WebGL2. A failed `connect` is called again, so an adapter's `connect` that rejects must leave nothing open (`caveaServer`'s does not, and forgets a failed event lookup). The realtime connection has its own reconnection (§4.1).

| Situation | Code, fatal | The buyer sees | The host gets | Retries |
|---|---|---|---|---|
| Invalid options | `invalid_option`, fatal | Nothing: the map is not mounted | Rejection | None |
| Outside a browser | `not_browser`, fatal | Nothing | Rejection | None |
| The chart, WASM or first state fails for a while, then loads | none | The loading skeleton throughout | Nothing; `createSeatMap` resolves | Up to three, quietly |
| …and still fails after the retries, with `ui.loading` (default) | `chart_fetch_failed`, `wasm_unavailable` or `state_connect_failed`, recoverable | The error state: `loadFailed` and Retry | `onError`; the promise stays pending (D-48) and resolves when a retry succeeds | Retry starts three more |
| …without `ui.loading` | The same codes, fatal | Nothing: the map is removed | Rejection | None more |
| Offline before the first frame | none while waiting | The error state with `offline`, no Retry | Nothing until it is online and a try fails | Paused; resumes when online |
| Page hidden before the first frame | none | The skeleton | Nothing | Paused; resumes when shown |
| A chart the server no longer has (404, 410), an unknown event or a refused key (4xx) | `chart_fetch_failed` or `state_connect_failed`, fatal | Nothing: the map is removed | Rejection | None |
| A chart that does not decode | `bad_chart`, fatal | Nothing | Rejection | None |
| WASM blocked by a Content Security Policy, broken, or not on the server (4xx); no WebAssembly | `wasm_unavailable`, fatal | Nothing (the list view, REN-071, A11Y-015, is not built) | Rejection | None |
| WebGL2 unavailable | `webgl_unavailable`, fatal | Nothing (the Canvas 2D fallback, REN-070, is not built) | Rejection | None |
| The host aborts `signal` before the map is ready | `aborted`, fatal | Nothing: the map is removed, the adapter disconnected | Rejection | Stopped |
| The host aborts `signal` after | none | Nothing: the map is destroyed | — | — |
| Glyph atlas fails | `glyph_atlas_failed` | The map without seat labels | `onError` | Three, quietly; then labels stay off |
| A background image fails (no source, a refused fetch, a 404, an undecodable or oversized image) | `image_failed` | The map without that image | `onError`, once per image | None: asked again only when the context is restored or another chart loads |
| GL context lost | `context_lost` after 5 s | No frame while lost; after 5 s the error state with `contextLost`, no Retry, until the context returns (04 §20.2: everything is made again from the engine's copies, no refetch) | `onError` once per loss | The browser's restore |
| Realtime connection drops | `state_disconnected` | The map stays usable; the `reconnecting` toast; holds still go over HTTPS | `onError` at the first drop | `caveaServer` reconnects with backoff from `seq` and resyncs (§4.1); the adapter does not report the reconnection, so the indicator is a toast that closes, not a lasting badge |
| Sequence gap | none | Nothing | Nothing | Automatic resync |
| Hold request fails for network reasons | `hold_failed` | The seat reverts; a toast | `onHoldFailed`, `onError` | The adapter's (`caveaServer` retries once, §4.1); the SDK reverts and reports a rejected `hold` |
| Unknown option | none | Nothing | A console warning in development builds (§3.6) | — |

The error state, the offline wait and the retries before the first frame are what the buyer sees of a failed start; a failure after the map is ready never replaces it (the atlas, the context, the realtime connection), except a lost context's notice; and the first frame does not end an error state that waits for Retry (the first state can fail after the chart is drawn). Measured in the tests (`packages/renderer/src/errors.test.ts`) with a fake platform: each row above produces its code, interface state and callback.

After `onReady`, the renderer never throws into host code: methods, callbacks, input and frames report through `onError`.

## 10. Loading and performance guidance

For integrators; the docs site repeats this.

- **Preload.** `preloadSeatMap({ assets?, chart? })` starts fetching and compiling the WASM, fetching and decoding the glyph atlas and, given `chart: { url }`, fetching the chart, before any container exists, as early as the route is known (on hover of a "Choose seats" link, say). A later `createSeatMap` with the same assets and chart URL uses them: the WASM is compiled once per page whoever asked first, and the atlas and the preloaded chart are shared by every later map. It returns a promise that resolves when everything has settled and never rejects; a failure is not kept, so a later map fetches again (and retries, §9). It is safe to call many times, and on a server: without a DOM it does nothing. Measured on the demo (`?chart=<stadium-100k.ccf>&wait=3000&preload=1|0`: the map created 3 s after the page starts; Chromium 157 headless, SwiftShader software rendering, a fresh profile for each run so the cache is cold, the files served by the Vite dev server on the same machine, so with no network latency): `first_frame`, from `createSeatMap` to the first frame with the chart, 145 to 160 ms without and 50 to 52 ms with (three runs each). On a real network the saving grows by the WASM's and the chart's download time, which preloading overlaps with the page's own work.
- **Preload hints.** In the page's `<head>`, before any script runs: `<link rel="preload" href="…/cavea_web_bg.wasm" as="fetch" type="application/wasm" crossorigin>`, `<link rel="preload" href="…/glyphs.png" as="fetch" crossorigin>` and, for a chart, `<link rel="preload" href="…/chart.ccf" as="fetch" crossorigin>`; and `<link rel="modulepreload" href="…">` for the bundle's entry chunk. `as="fetch"` with `crossorigin` matches how the map fetches them (`fetch()` in CORS mode), so the browser reuses the preloaded response; a different `as` or a missing `crossorigin` downloads the file twice. A bundler renames the WASM and the atlas with a content hash (`cavea_web_bg-<hash>.wasm`): take the hashed names from its manifest (Vite's `build.manifest`, webpack's `assets-manifest`), or pass `assets.baseUrl` to serve them from a fixed place and preload those URLs.
- **Background images** (04 §7) never delay the map: the engine asks for them after the first frame, `createSeatMap` resolves without waiting for them, and each appears when it has been fetched and decoded, at most two loading at once. The map fetches the URL `assets.image` or the adapter gives (`caveaServer`: `/v1/public/assets/{hash}` on its server) with `fetch()` in CORS mode, so the image's origin needs CORS (the server's public routes send `Access-Control-Allow-Origin: *` and `Cross-Origin-Resource-Policy: cross-origin`) and a place in `connect-src` (below), not `img-src`. The browser decodes it (`createImageBitmap`), and one larger than the engine's limits (4,096 px a side, 16 MiB for the floor's images together) is scaled down before it is uploaded; the bitmap is closed once uploaded. A failure is `image_failed` and the map carries on. Measured locally (2026-11-11; AMD Ryzen 9 7900X; headless Chromium 155, SwiftShader; `apps/demo` against a release `cavea-server` on another origin over loopback, a fresh profile per page; the `theatre` preset with and without a 1,200 × 800 PNG behind it): `state_ready` 71, 72 and 197 ms without the image and 72, 82 and 83 ms with it, `first_frame` 100 to 279 ms and 135 to 149 ms; the image's fetch started at or after `first_frame` every time (it is asked for after the first frame), so it cannot move either mark. Served 3 s late, it appeared 3 s after a map that was already drawn and usable.
- **Self-host assets** on the same origin or a CDN with long cache lifetimes; all files are content-hashed.
- **Content Security Policy.** The smallest policies, found by starting from `default-src 'none'` and adding one source at a time until the map draws, then taking each away again (`fixtures/run.ts`, `csp-*` checks; 13 §8 has the results):

  | Set-up | Policy |
  |---|---|
  | `index.js` (bundled, or as a module) with a custom adapter, files on the page's origin | `default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'` |
  | `caveaServer` against a server on another origin | the above, with `connect-src 'self' https://seats.example.com wss://seats.example.com` |
  | `cavea.min.js` from a CDN | `default-src 'none'; script-src 'self' https://cdn.example.com 'wasm-unsafe-eval'; connect-src 'self' https://cdn.example.com` |

  Each source and why: `script-src` the scripts' origins, and `'wasm-unsafe-eval'` to compile WebAssembly (on browsers that predate it, `'unsafe-eval'` is the only way); `connect-src` every origin the map fetches from, since the WASM, the atlas, the chart, the event and its state, and background images are all fetched with `fetch()` (a chart or an image on another origin adds that origin; `caveaServer`'s images come from its server, already there); the server's `wss:` (or `ws:`) origin for the WebSocket, which an `https:` source does not cover. EventSource and polling are `https:` requests. The page's own scripts and styles need what they need; the map needs nothing else: no `img-src` (the atlas is decoded from a fetched blob), no `style-src`, `font-src`, `worker-src` or `'unsafe-inline'`, and no `'unsafe-eval'` where `'wasm-unsafe-eval'` is supported. **What fails without a source**, and how the map says so: without `'wasm-unsafe-eval'`, `createSeatMap` rejects at once with `wasm_unavailable` (fatal, never retried); without the files' origin in `connect-src`, the refused fetch is a network failure, retried three times (§9), then `wasm_unavailable` or `chart_fetch_failed` (whichever fetch is refused first; with `ui.loading` the error state and a recoverable error instead of a rejection); without the server's origin, `chart_fetch_failed` or `state_connect_failed`; without its `wss:` origin, the map works and `caveaServer` falls back to EventSource after two refused sockets (`transport: 'auto'`; a forced `'ws'` keeps retrying). Every refusal also fires the page's `securitypolicyviolation` event. **Styles** need nothing: the interface's stylesheet is one constructed `CSSStyleSheet` adopted by the shadow root (`adoptedStyleSheets`), and positions, colours and tokens are set through the CSSOM (`element.style.setProperty`), neither of which `style-src` governs. Browsers without constructable stylesheets (Safari before 16.4) get a `<style>` element instead, which a policy without `style-src 'unsafe-inline'` (or a nonce, not supported yet) blocks: the map then works unstyled. The colour reading of §8 and the text rasteriser draw on canvases, which need no permission. **Trusted Types:** every policy above was also run with `require-trusted-types-for 'script'` and reported no violation: the SDK never builds HTML or script from strings (no `innerHTML`, `eval`, `Function` or script URLs; text is set as text), so it needs no Trusted Types policy of its own. A host's tooltip function that returns an element built with `innerHTML` is the host's to make compliant.
- **Server-side rendering.** Importing the package on the server is safe; nothing touches `window` until `createSeatMap` runs, which rejects with `not_browser` outside a browser. Checked with Next.js (13 §8): a server component page with a client component using `@cavea/react`, and one that imports `@cavea/renderer` in an effect with `import()`, both build, render on the server and hydrate.
- **Bundlers.** See §13: Vite, webpack 5 and Next.js need nothing; Rollup and esbuild do not copy files named with `new URL(…, import.meta.url)`, so the WASM and the atlas are copied beside the bundle (or served elsewhere and named by `assets.baseUrl`).
- **Size the container** before creating the map. The renderer observes later resizes.
- **Hiding the container** with `display: none` during creation works: `createSeatMap` still resolves, and the first frame is drawn when the container gets a size.

## 11. Framework wrappers

```tsx
import { SeatMap } from '@cavea/react';

<SeatMap
  state={adapter}
  pricing={pricing}
  selection={{ max: 8 }}
  onSelectionChange={setSelection}
  ref={mapRef}            // exposes the imperative methods
  style={{ height: 520 }}
/>
```

Rules for every wrapper:

- Props map one-to-one to options; changing a prop calls the matching setter and never recreates the map.
- Changing `state` or `chart` identity recreates the map; wrappers warn in development if that happens on every render.
- Unmounting calls `destroy()`.

**`@cavea/react`** (D-59). `<SeatMap>` takes every option of §3 and callback of §5 as a prop, except `signal` (the mount is the map's lifetime), plus `id`, `className` and `style` for the container `<div>`. React 18 and 19 are peer dependencies.

- **Rendering** returns the empty `<div>` and nothing else, on the server too: the map is created in an effect, a microtask after mount, so React's Strict Mode (mount, unmount, mount in one go) cancels its first mount before anything is fetched or connected, and one map is made.
- **Setters.** After each render, `pricing`, `theme` and `locale` that changed in value (compared as data, so a new object or array with the same content is no change) go to `setPricing`, `setTheme` and `setLocale`; taken away, they go back to the default (`[]`, `'auto'`, the browser's language). Other options have no setter: a change is ignored with a development warning, and applies when the map is next created. `initialView` is read at creation only, without a warning.
- **Callbacks** are passed to the map as stable functions that call the latest prop, so a new function every render costs nothing and a callback added later is called. Not `onMetrics`, `onError` and `formatPrice`, whose presence changes what the map does (frame timing, development warnings, the price format): they count from creation, and adding or removing one later warns. A `ui.tooltip` function is also the latest prop's.
- **Recreating.** A new `state` adapter, or a `chart` with another URL (compared as text) or other bytes (by identity), destroys the map and creates another; a new adapter on three renders in a row warns, since it usually means `caveaServer(…)` is called in render: make it once (`useMemo`, or outside the component).
- **The ref** is the map (§6) once `createSeatMap` resolves, `null` before, while it is recreated and after unmount.
- **Unmounting** aborts the map's `signal`: a map that is ready is destroyed, one still loading is cancelled (`aborted`, which the component swallows). A start that fails otherwise goes to `onError`, or is rethrown as an unhandled rejection without one.
- **A state adapter serves one map** (§4: `disconnect` is last), so a component that mounts again with the same adapter object (a parent that unmounts and mounts it, or React hiding and showing it with `<Activity>`) needs a new one: key the adapter's `useMemo` on whatever remounts it. Strict Mode does not, as above.

## 12. Modes

| Mode | Purpose | Interaction |
|---|---|---|
| `picker` | Buying | Select, hold |
| `static` | Showing a layout | Pan and zoom only |
| `spotlight` | "Your seats" on confirmation pages and in apps | Given labels are highlighted, everything else is dimmed; camera fits them |
| `manager` | Back office | Multi-select by click, drag box and row; shows true statuses; needs a manager session |

## 13. Compatibility

| Environment | Support |
|---|---|
| Chrome, Edge, Firefox, Safari: current and previous two major versions | Full renderer |
| iOS Safari 15 and later | Full renderer |
| Browsers without WebGL2 | Canvas 2D fallback |
| No canvas, no WASM, or scripting restricted | List view |

Compatibility is tested per release (`13-testing-benchmarks.md` §8).

**Bundlers and frameworks** (checked by `fixtures/run.ts`, each consuming the packed package as a customer would; versions and results in 13 §8):

| Tool | Configuration needed |
|---|---|
| Vite (build and dev server) | None: it copies the WASM and the atlas, renamed with a hash, and the dev server serves them from the package |
| webpack 5 | None: `new URL(…, import.meta.url)` makes both files assets, renamed with a hash, found through `publicPath: 'auto'` |
| Next.js (App Router; Turbopack) | None, for `@cavea/react` in a client component or `import('@cavea/renderer')` in an effect |
| Rollup | Copy `cavea_web_bg.wasm` and `glyphs.png` from `@cavea/renderer` beside the output: Rollup copies no file named by `new URL(…, import.meta.url)`. Six lines of plugin (`this.emitFile` of both, `fixtures/rollup/rollup.config.js`), or `@web/rollup-plugin-import-meta-assets`, or a copy step. Rollup also needs `@rollup/plugin-node-resolve`, as for any package |
| esbuild | `--format=esm` (an IIFE has no `import.meta.url`), and copy both files beside the output: esbuild leaves the `new URL` as it is |
| No bundler | `cavea.min.js` by a script tag, or `index.js` by `<script type="module">` (or an import map), with the two files beside it |

Instead of copying, any of them can serve the two files from a fixed place and pass `assets.baseUrl`. None inlines the WASM as base64 into the JavaScript.

## 14. Example: bring your own backend

```ts
const state = customState({
  async connect(ctx) {
    const res = await fetch(`/api/events/${eventId}/seats`);
    const { unavailable, mine } = await res.json();
    events.onmessage = (m) => ctx.applyDelta(JSON.parse(m.data));   // { unavailable: [...], available: [...] }
    return { kind: 'map', unavailable, mine };
  },
  async hold(labels) {
    const res = await fetch(`/api/cart/seats`, { method: 'POST', body: JSON.stringify({ labels }) });
    return res.ok ? { ok: true } : { ok: false, failed: await res.json() };
  },
  async release(labels) {
    await fetch(`/api/cart/seats`, { method: 'DELETE', body: JSON.stringify({ labels }) });
  },
  disconnect() { events.close(); },
});
```

This path uses no Cavea server. It is the "renderer only" licence.

## 15. Server SDK

`@cavea/server-sdk` is the server's REST API (08 §6) for a customer's backend: Node 20.3 or later (`AbortSignal.any`) and bun, on the platform's `fetch`. It holds a secret key, so it is never used in a browser: constructed where `window` and `document` exist, it throws unless `dangerouslyAllowBrowser` is set (tests that fake a browser only). The page talks to the customer's backend, which talks to Cavea.

```ts
import { Cavea, isCaveaError } from '@cavea/server-sdk';

const cavea = new Cavea({ baseUrl: 'https://seats.example.com', secretKey: process.env.CAVEA_SECRET_KEY, keyId: process.env.CAVEA_KEY_ID });

const token = await cavea.holdTokens.create({ event: 'hamlet-2027-03-14' });
await cavea.events.hold('hamlet-2027-03-14', { hold_token: token.hold_token, objects: ['Orchestra-C-12'] });
// After payment: the order number as the idempotency key, so a retry books once.
await cavea.events.book('hamlet-2027-03-14', { hold_token: token.hold_token, order_ref: '77120' }, { idempotencyKey: 'order-77120' });

for await (const seat of cavea.events.listObjects('hamlet-2027-03-14', { status: 'booked' })) { /* every booked seat once */ }

try { await cavea.events.hold('hamlet-2027-03-14', { hold_token: token.hold_token, objects: ['Orchestra-C-13'] }); }
catch (e) { if (isCaveaError(e, 'object_not_free')) { /* e.details.objects names the seats */ } else throw e; }

const session = await cavea.signSessionToken({ sub: buyerId, event: 'hamlet-2027-03-14' });   // for the picker's Cavea-Session (08 §11)
```

**What it is made of** (D-77). `cargo xtask types` writes `src/generated.ts` from the committed OpenAPI document (`xtask/src/api.rs`, the generator the console uses): a type for each schema the SDK's operations reach, each operation's input and reply, and a table of the operations a secret key or the admin key may call, with method, path template, credential, whether it takes an `Idempotency-Key` and whether it is a page. `cargo xtask types --check`, and so `cargo test -p xtask`, fails while the file is stale. One hand-written request layer (`src/core.ts`) turns the table into calls; a thin layer (`src/index.ts`) groups them by resource. `cavea.call(operationId, input)` reaches every operation of the table, typed, so an operation the server gains is callable as soon as the types are regenerated. `src/wrappers.ts` names the method that reaches each operation, typed against the table: an operation without a line fails `bun run typecheck` until it gets a method or `null` (reachable through `call` only, as `metrics` and `eventState`, the manager's binary frames, are); `wrappers.test.ts` calls each method and checks the method and path it sends.

| Group | Methods |
|---|---|
| `charts` | `list`, `create`, `get`, `archive`, `getDraft` (the document and its `ETag`), `saveDraft` (`ifMatch` required: the etag read, or `"*"`), `validate`, `publish` (`ifMatch` optional; `created` false when the draft compiled to a version the chart had), `versions`, `setThumbnail`, `getLock`, `lock`, `unlock` (the designer's editing lock, 07 §9) |
| `events` | `list` (`chart`, `q`, `archived`, `order` to filter), `create`, `get` (with counts), `update`, `archive`, `hold`, `release`, `book`, `changeStatus`, `objects` (by label: `{data, unknown}`), `listObjects` (`status` to filter) |
| `holdTokens` | `create`, `get`, `extend`, `release` |
| `keys` | `list`, `create`, `revoke`, `updateOrigins` |
| `assets` | `upload` (bytes; `created` false when the workspace had them) |
| `workspaces` (admin key) | `list`, `create`, `mintKey`, `inviteOwner` |

**Rules it applies.**

- **Credentials.** `secretKey` (`sk_` and 64 hex digits, checked at construction) for workspace operations, `adminKey` (32 to 512 printable ASCII characters without spaces, the server's rule, checked at construction so that no platform error ever prints it) for the operator's; an operation whose key the client lacks fails before sending. The key goes in `Authorization: Bearer`, never a URL, and is kept in a closure: printing the client or an error never shows it.
- **Idempotency keys** (08 §6.1). Every call to an operation that takes one (hold, release, book, change status, extending and releasing a hold token) sends one: the caller's `idempotencyKey` (1 to 255 printable ASCII characters without a space at either end, which a header value would lose, so that two keys never become one; checked before sending), else a random UUID made for that call. Retries of the call send the same key, so a reply lost on the way back is replayed, not applied twice. The same key with another request is `idempotency_mismatch`. An `idempotencyKey` on an operation that takes none is a `TypeError`.
- **Retries, only where a repeat cannot do something twice.** After a network failure, a timeout, `429`, `500`, `502`, `503` or `504`, the client tries again (2 retries by default, `maxRetries`) only for reads (`GET`), archives and revocations (`DELETE`, idempotent by 08 §6.2), a draft saved over `If-Match: *`, and calls carrying an idempotency key. Never a create, a publish, an event change, a hold-token creation or a draft saved over an etag (its repeat would conflict with itself). It waits `Retry-After` when the answer gives one, and fails at once with it when it is longer than `maxRetryAfter` (30 s by default); otherwise a jittered back-off from 250 ms, doubling. Any other `4xx` is final.
- **Replies** are the operation's JSON, or text (`metrics`), or bytes as an `ArrayBuffer` (`eventState`'s frames), or `undefined` where the document allows an empty success; anything else on a `2xx` is `unexpected`.
- **Timeouts and cancellation.** Each attempt has `timeoutMs` (30 s by default; per call too); `signal` cancels the whole call, retries and waits included, and the call rejects with the signal's reason.
- **Pages** are async iterators that follow `next` exactly until it is `null`: an empty page with a `next` is not the end (an objects page looks at 10,000 objects at most, 08 §6.2), and a cursor returned twice is an error rather than a loop. `.pages()` gives the pages.
- **Errors** are one class, `CaveaError`: `code` (the problem's, 08 §6.4, typed from the document's catalogue; or `network`, `timeout`, `unexpected` for an answer that is not one of the API's: a failure that is not a problem, or a success without the operation's JSON, such as a proxy's page, so that a call that never reached Cavea never looks done), `reason` and `details` as the problem gives them, `status`, `requestId` (from `Cavea-Request-Id`), `retryAfter` and `operation`. Its message is built from those alone: never the key, the request's body, headers or URL (a hold token is in some paths), nor the server's free-text message, which can quote part of a refused body. `reason` is a string: the document does not enumerate reasons.
- **Session tokens** (08 §11, D-34): `signSessionToken({sub, event, ttlSeconds?, maxObjects?, channels?})` signs HS256 with the secret key, `kid` the key's id (`keyId`, from the key's creation or `keys.list()`); the server verifies it with the key's stored SHA-256, which is the same HMAC. A token over the server's 1,024 bytes is refused before it is returned.
- `baseUrl` may carry a path prefix (a proxy at `/cavea/`) and no credentials, query or fragment; redirects are refused, so a key is never sent on to another place.

**Packaging** follows `@cavea/renderer` (D-58): `bun run build` in `packages/server-sdk` writes `dist/` (`index.js`, one ES module with no imports; `index.d.ts` and the declarations it reaches; `package.json` with `"type": "module"`, `"sideEffects": false`, `engines.node >=20.3`), and fails while `api.md`, the public declarations as one file, is stale (`bun run build --update-api`). No runtime dependencies. There is no script-tag build or WASM: it is not for pages.

**Proof.** `src/client.test.ts` holds the rules above to a scripted `fetch`. `test/e2e.test.ts` runs against a real `cavea-server` on a database of its own when `CAVEA_TEST_DATABASE_URL` is set: a workspace by the admin key, a chart published, events walked in pages, hold, book, an idempotent replay and a mismatch, a conflict, draft etags, a lost reply retried and applied once, a `429` waited for (made by a wrapping `fetch`: secret-key routes are not rate limited, 08 §12), and a session token the server's public route accepts. `bun fixtures/run.ts server-sdk` installs the packed tarball into a strict TypeScript project (no DOM library, `skipLibCheck` off) and runs it under Node 22 and 24 against `cavea-server` (13 §8).
