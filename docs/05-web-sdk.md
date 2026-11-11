# 05 · Web SDK

**Status:** Draft v0.1 · 2026-11-10

The public JavaScript surface of the renderer. Everything here is semver-stable from 1.0.

## 1. Packages

| Package | Contents |
|---|---|
| `@cavea/renderer` | Framework-agnostic renderer: TypeScript API, WASM, font atlas, default styles |
| `@cavea/react`, `@cavea/vue`, `@cavea/svelte` | Thin wrappers (props in, events out) |
| `@cavea/designer-embed` | Helper for embedding the designer in an iframe |
| `@cavea/manager-embed` | Helper for embedding the event manager |
| `@cavea/server-sdk` | Node client for the server's REST API (never used in the browser) |

A script-tag build (`cavea.min.js` plus `cavea.wasm`) is published for sites without a bundler (not built yet: WEB-013).

**Built today** (WEB-001 to WEB-005): `createSeatMap`, `staticState`, `customState`, `CaveaError` and their types, from `@cavea/renderer`, which has no runtime dependencies. Features this document describes but that are not built yet are marked "not yet" with the task that builds them. Options and callbacks that are not built are left out of the TypeScript types, so a typed build fails on them; passed anyway, they are ignored with a development warning (§3.6). `loadRenderer()` loads the shared WASM and reports its `version()`; nothing else below the public API is exported.

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

Not yet: `caveaServer` (WEB-020), `pricing` and `formatPrice` (WEB-009) and `selection.validators`. Without a server, `customState` (§14) works today.

## 3. Options

### 3.1 Data

| Option | Type | Default | Notes |
|---|---|---|---|
| `chart` | `{ url }` or `{ bytes }` | From the state adapter | CCF location; `url` a string or `URL`, `bytes` an `ArrayBuffer` or `Uint8Array` |
| `state` | `StateAdapter` | `staticState()` | See §4 |
| `pricing` | `CategoryPricing[]` | none | Display only; prices in minor units. Not yet (WEB-009) |
| `formatPrice` | `(minor, ctx) => string` | Plain number | Not yet (WEB-009) |
| `floor` | floor id | First floor | Initial floor. Not yet: only the first floor is drawn |

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
| `selection.restore` | boolean | true | Restore the buyer's holds after a reload. Not yet as an option (WEB-023): the engine always restores the holds an adapter reports as the buyer's (`mine`, or a PRIVATE frame) |

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
| `ui.zoomControls` | boolean | true | Not yet (WEB-006) |
| `ui.floorSelector` | boolean | true when more than one floor | Not yet (WEB-030) |
| `ui.legend` | boolean | true | Not yet (WEB-006) |
| `ui.filters` | `{ categories?, price?, accessible? }` or false | false | Not yet (WEB-030) |
| `ui.tooltip` | boolean or `(obj) => HTMLElement \| string` | true | Not yet (WEB-007) |
| `ui.holdTimer` | boolean | false | Not yet (WEB-024) |
| `ui.listViewToggle` | boolean | true | Not yet |
| `theme` | `'light'`, `'dark'`, `'high-contrast'`, `'auto'`, or `{ base?, tokens }` | `'auto'` | Built. `'auto'` is light or dark following `prefers-color-scheme`, and its changes. `tokens` maps §8 names (with or without `--cavea-`) to values over `base` (default `'light'`; `'auto'` allowed); an unknown token or bad value is reported through `onError` (`invalid_input`) and the theme is unchanged |
| `colorblind` | boolean | false | Not yet |
| `locale` | BCP 47 tag | `navigator.language` | Not yet (WEB-011) |
| `messages` | Partial message catalogue | built-in | Not yet (WEB-011) |
| `dir` | `'ltr'`, `'rtl'`, `'auto'` | `'auto'` | Not yet |

### 3.5 Loading

| Option | Type | Default | Notes |
|---|---|---|---|
| `assets.baseUrl` | string or `URL` | Package location | Where `cavea_web_bg.wasm` and `glyphs.png` are served from; relative to the page if relative |
| `assets.wasm` | `Response`, `ArrayBuffer`, `Uint8Array`, `WebAssembly.Module`, or URL | — | Bring your own fetch. The WASM is loaded once per page: the first map's source is used by every later map |
| `fallback` | `'auto'`, `'canvas'`, `'list'` | `'auto'` | Force a step of the capability ladder. Not yet (WEB-017) |
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
  reportError(error: unknown): void;                  // to onError, not fatal
}
```

How it works:

- **Two forms of state.** An adapter speaks either the server's wire format (`binary` snapshots and `applyFrame`, handed to the engine byte for byte; HELLO, RESYNC, PING and CLOSE frames are the adapter's own business) or labels (`map` snapshots, `applyDelta`, `setMine`). Label state is converted to the engine's per-object codes inside the WASM in one call per update (`View.setSnapshotLabels`, `applyDeltaLabels`, `setPrivateLabels`), never one call per seat. Label deltas are numbered by the SDK, so they never trip a resync; an adapter does not mix the two forms. Unknown labels are skipped and reported through `onError` (`state_failed`).
- **`mine` and `expiresAt`.** Labels the buyer's hold token holds; the engine keeps them selected, selects them after a reload, and reports `expiresAt` in `onSelectionChange`. A `map` snapshot without `mine` leaves the buyer's set as it was.
- **Order.** Calls on the context made before the first snapshot is applied (before the chart is loaded, or before `connect` resolves) are kept and applied after it, in order. A snapshot that arrives before the chart is applied once the chart is decoded.
- **Holds.** With `hold` and `release`, each engine `HoldRequest` and `ReleaseRequest` (04 §16) is passed to the adapter. `hold` and `release` are called one at a time, in the order the engine emits them, each after the previous call's promise has settled; the engine already coalesces taps into at most one hold in flight. `{ ok: true }` holds every label of the request. A refusal is all or nothing (02 §4.3): `failed` names the objects at fault with their reasons (the server's per-object `reason`, such as `object_not_free`), and `code` the request's (such as `hold_limit_exceeded`, `rate_limited`, or `not_found` for a hold token that is gone, which drops the whole selection); objects `failed` does not name are requested again with the next hold. A rejected or thrown `hold`, or one without an answer in 20 seconds, is reported as `hold_failed` through `onError` and fails the request with code `hold_failed`. A failing `release` is reported as `release_failed`. A result after `destroy` is ignored. Retrying a request is the adapter's choice (`caveaServer` retries once, WEB-021).
- **Manual session.** An adapter without `hold` and `release` puts the engine in `manual` session mode: taps select at once and nothing is held (D-14).
- **Lifetime.** `chart` and `connect` are called once, at start. `disconnect` is called once, last, on `destroy` or when loading fails; no other method is called after it, and the context's calls do nothing.
- **Errors.** A rejected `connect`, or a first state the engine refuses (a malformed frame, a snapshot of the wrong length), rejects `createSeatMap` (`state_connect_failed`): a picker without state would offer seats it cannot know are free. Unknown labels in it are only reported. Every later adapter error goes to `onError` and never reaches the page as an exception or an unhandled rejection.

What `caveaServer` (WEB-020 to WEB-025) uses: `chart` returns the event info's `chart.url` (fetched with the snapshot in parallel with the WASM); `connect` creates or restores the hold token (`setHoldToken`), fetches the binary snapshot and resolves with it, then subscribes with the snapshot's `seq` and passes SNAPSHOT, DELTA and PRIVATE frames to `applyFrame`; `hold` and `release` send the token in the JSON body and map a problem response to `{ ok: false, code, failed }` from the failing objects' `label` and `reason`; `resync` fetches a fresh snapshot for `applySnapshot`; `pause` and `resume` drop and restore the realtime connection; `disconnect` closes it; `signal` cancels its fetches.

Built-in adapters:

| Adapter | Use |
|---|---|
| `caveaServer(options)` | Talks to `cavea-server`: binary snapshot over HTTP, deltas over WebSocket with SSE and polling fallback, holds over HTTPS. Not yet (WEB-020) |
| `staticState({ unavailable })` | Fixed state by label, no holds, for static and spotlight modes or demos |
| `customState(impl)` | The host's own backend. Implement `connect`, and `hold` and `release` if you want hold-on-select |

`caveaServer` options:

| Option | Type | Default | Notes |
|---|---|---|---|
| `baseUrl` | string | — | |
| `publishableKey` | string | — | |
| `event` | string | — | Event key |
| `session` | `'start'`, `'continue'`, `'manual'`, `'none'` | `'start'` | `start` creates a hold token; `continue` reuses one |
| `holdToken` | string | — | For `continue` |
| `sessionToken` | string or `() => Promise<string>` | — | Signed by the customer's backend (`08-server.md` §11); carries the buyer's `sub`, limits and channels. Sent as `Cavea-Session` on hold-token creation, hold and release, and in SUBSCRIBE. Required before the first hold when the event info says `require_session` |
| `transport` | `'auto'`, `'ws'`, `'sse'`, `'poll'` | `'auto'` | |
| `pauseWhenHidden` | boolean | true | Drops the realtime connection in background tabs and resyncs on return |

The hold token is persisted in `sessionStorage` under a key derived from the event, so a reload keeps the buyer's seats.

## 5. Callbacks

| Callback | Fires when | Payload | Today |
|---|---|---|---|
| `onReady` | Chart-ready | `{ chart: { name, seats, objects }, backend: 'webgl2', marks }`: timing marks in ms since `createSeatMap` (04 §24: `init_start`, `wasm_ready`, `chart_fetched`, `chart_decoded`, `first_frame`, `state_ready`; also `performance.mark('cavea:<name>')`) | Built; fires just before the promise resolves. `first_frame` is missing while the container is hidden |
| `onSelectionChange` | The selection changes | `{ objects, holdToken, valid, expiresAt }` | Built; `valid`: at least `selection.min` objects |
| `onObjectSelect` / `onObjectDeselect` | One object is added or removed | Object | Built |
| `onSelectionInvalid` / `onSelectionValid` | Rule state changes | `(rule, objects)` / — | Built: rule `min` (objects: the selection) or a refusal (`max`, `not_free`, a hold code: objects refused) |
| `onHoldPending` / `onHoldSettled` | Hold requests start and finish | — (use to disable the checkout button) | Built: pending when the first of a run of holds starts, settled when the last is answered |
| `onHoldFailed` | A hold was refused | `[{ object, reason }]` | Built |
| `onHoldLost` | The buyer's hold on selected objects is gone (expired, released elsewhere, token gone) | Objects; they also leave the selection | Built (new) |
| `onObjectUnavailable` | A selected object was taken | Object | Built |
| `onHoldExpiring` | 60 s before expiry | Seconds left | Not yet (WEB-024) |
| `onHoldExpired` | The hold token expired | — | Not yet (WEB-024) |
| `onObjectHover` / `onObjectFocus` | Pointer or keyboard focus | `(object or null, at)`: `at` the object's centre in CSS pixels | Built; focus waits for keyboard navigation (A11Y-001) |
| `onSectionEnter` | The camera drills into a section | Section | Not yet |
| `onFloorChange` | The floor changes | Floor | Not yet |
| `onCameraChange` | Pan or zoom settles | `{ centre, zoom, level }` | Built |
| `onMetrics` | At most once a second | Frame statistics | Not yet |
| `onError` | Any recoverable or fatal error | `CaveaError` | Built |

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

Today `kind` is always `'seat'` (tables, booths and areas are not compiled yet), `floor` is the floor name of the object's section (the first floor for a seat outside sections), and `ticketType`, `quantity` and `price` are not in the type until pricing exists (WEB-009). `section`, `row` and `seat` are left out when the chart has none.

## 6. Methods

| Method | Notes | Today |
|---|---|---|
| `map.getSelection()` | Current objects and hold token | Built: `{ objects, holdToken, valid, expiresAt }` as `onSelectionChange` |
| `map.select(labels)`, `map.deselect(labels)`, `map.clearSelection()` | Subject to the same rules as taps | Built; all or nothing; unknown labels are reported (`invalid_input`) |
| `map.selectBestAvailable({ quantity, categories?, accessible? })` | Uses the adapter's `bestAvailable` | Not yet |
| `map.setPricing(p)`, `map.setFilter(f)`, `map.setTheme(t)`, `map.setLocale(l)` | Live updates | `setTheme` built (§3.4 values); the others not yet (WEB-009, WEB-030, WEB-011) |
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

`createSeatMap` returns a promise that resolves at chart-ready (the chart decoded and the first state applied; it does not wait for a frame, so a hidden container does not hold it up) and rejects only if no step of the capability ladder works. The ladder (Canvas 2D, then the list view) is not built yet (WEB-017): today WebAssembly and WebGL2 are required, and without them the promise rejects with `wasm_unavailable` or `webgl_unavailable`. The ladder hooks in where `createSeatMap` creates the WebGL2 view. It also rejects for invalid options, outside a browser (`not_browser`), and when the chart or the first state cannot be had (§9).

## 7. Built-in interface

All built-in controls live in a shadow root on the container, so host styles cannot break them and theirs cannot leak.

Today the shadow root holds only the canvas, which fills the container (`role="img"`, `aria-label="Seat map"` until the accessibility layer, A11Y-001), and the controls below are not built (WEB-006 to WEB-008). The container's own children are not displayed while the map is in it. The container is `display: block` unless the page styles it otherwise; the `hidden` attribute still hides it.

| Element | Notes |
|---|---|
| Zoom controls | In, out, fit. Keyboard reachable |
| Floor selector | Segmented control; a menu above five floors |
| Legend | Categories with colour, shape cue and price; states (available, selected, unavailable) |
| Filters | Category chips, price range, "accessible only" |
| Tooltip | Seat, row, section, category, price, ticket-type choice, flags. Replaceable by the host |
| Area sheet | Quantity stepper for general-admission areas |
| Toasts | "That seat was just taken", rule violations |
| Hold timer | Optional countdown with an "I need more time" action that calls the host |
| List-view toggle | Switches to the accessible list |
| Loading and error states | Skeleton of the chart bounds; retry action |

Each can be disabled, and each exposes `part` names for `::part()` styling.

## 8. Theme tokens

Set as CSS custom properties on the container or passed as `theme`.

| Token | Meaning |
|---|---|
| `--cavea-bg` | Canvas background |
| `--cavea-fg`, `--cavea-fg-muted` | Text |
| `--cavea-seat-available-stroke` | Stroke of available seats (fill is the category colour) |
| `--cavea-seat-unavailable-fill`, `--cavea-seat-unavailable-stroke` | |
| `--cavea-seat-selected-fill`, `--cavea-seat-selected-ink` | |
| `--cavea-ring-hover`, `--cavea-ring-focus` | |
| `--cavea-section-tint` | Strength of the overview tint, 0–1 |
| `--cavea-dim` | Opacity of filtered-out objects |
| `--cavea-font` | Font family for dynamic text and controls |
| `--cavea-radius`, `--cavea-shadow` | Controls |
| `--cavea-category-<key>` | Override one category's colour |

Token changes apply on the next frame without reloading the chart.

## 9. Errors and degradation

```ts
class CaveaError extends Error {
  code: CaveaErrorCode;  // stable, below
  fatal: boolean;        // fatal errors reject createSeatMap; after it resolves every error is recoverable
  option?: string;       // for invalid_option
  cause?: unknown;
}
```

Codes: `invalid_option`, `not_browser`, `wasm_unavailable`, `webgl_unavailable`, `chart_fetch_failed`, `bad_chart` and `state_connect_failed` (fatal, at start); `state_failed` (an adapter error, refused or unknown state), `hold_failed`, `release_failed`, `callback_failed`, `glyph_atlas_failed` (seats are drawn without labels), `invalid_input` (a method's input refused) and `engine_failed` (a frame or input call failed). `state_disconnected` comes with `caveaServer` (WEB-020).

| Situation | Behaviour |
|---|---|
| Chart fetch fails | Retry three times with backoff, then show the error state with a retry action. Today: `createSeatMap` rejects with `chart_fetch_failed` (WEB-017) |
| WASM blocked or fails to compile | Fall to the list view if the adapter can supply sections and rows; otherwise fatal. Today: fatal, `wasm_unavailable` |
| WebGL2 unavailable | Canvas 2D fallback. Today: fatal, `webgl_unavailable` |
| Glyph atlas fails | The map works without seat labels; `onError` with `glyph_atlas_failed` |
| GL context lost | Pause, restore, re-upload; no user-visible error if restored within 5 s |
| Realtime connection drops | Keep the map usable, show a discreet "reconnecting" indicator, resync on return; holds still go over HTTPS |
| Sequence gap | Automatic resync |
| Hold request fails for network reasons | Retry once, then revert the selection and report `onHoldFailed`. The retry is the adapter's (WEB-021); the SDK reverts and reports a rejected `hold` |
| Unknown option | Console warning in development builds only (§3.6) |

After `onReady`, the renderer never throws into host code: methods, callbacks, input and frames report through `onError`.

## 10. Loading and performance guidance

For integrators; the docs site repeats this.

- **Preload.** Add `<link rel="preload" as="fetch" crossorigin>` for the WASM and the chart, or call `preloadSeatMap(options)` as early as the route is known.
- **Self-host assets** on the same origin or a CDN with long cache lifetimes; all files are content-hashed.
- **Content Security Policy.** WASM compilation needs `script-src 'wasm-unsafe-eval'`. The renderer uses no `eval` and no inline scripts. Realtime needs the server's origin in `connect-src`.
- **Server-side rendering.** Importing the package on the server is safe; nothing touches `window` until `createSeatMap` runs, which rejects with `not_browser` outside a browser.
- **Bundlers.** The WASM and the atlas are referenced with `new URL('../wasm/cavea_web_bg.wasm', import.meta.url)` and `new URL('../assets/glyphs.png', import.meta.url)`, which Vite, webpack 5, Rollup and esbuild resolve.
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
