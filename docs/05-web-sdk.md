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

A script-tag build (`cavea.min.js` plus `cavea.wasm`) is published for sites without a bundler.

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

## 3. Options

### 3.1 Data

| Option | Type | Default | Notes |
|---|---|---|---|
| `chart` | `{ url }` or `{ bytes }` | From the state adapter | CCF location |
| `state` | `StateAdapter` | `staticState()` | See §4 |
| `pricing` | `CategoryPricing[]` | none | Display only; prices in minor units |
| `formatPrice` | `(minor, ctx) => string` | Plain number | |
| `floor` | floor id | First floor | Initial floor |

### 3.2 Selection

| Option | Type | Default | Notes |
|---|---|---|---|
| `selection.max` | number or `{ total, perTicketType }` | 10 | |
| `selection.min` | number | 0 | Checkout is "invalid" below this |
| `selection.groupSize` | number | 1 | One tap selects this many adjacent seats |
| `selection.validators.noOrphans` | `'off'`, `'lenient'`, `'strict'` | `'off'` | |
| `selection.validators.consecutive` | boolean | false | |
| `selection.categories` | string[] | all | Only these categories are selectable |
| `selection.selectable` | `(obj) => boolean` | none | Extra host rule; called on tap only |
| `selection.restore` | boolean | true | Restore the buyer's holds after a reload |

### 3.3 Behaviour

| Option | Type | Default |
|---|---|---|
| `mode` | `'picker'`, `'static'`, `'spotlight'`, `'manager'` | `'picker'` |
| `spotlight` | labels to highlight in spotlight mode | — |
| `gestureHandling` | `'greedy'`, `'cooperative'`, `'none'` | `'greedy'` |
| `zoomToSelect` | boolean | true on touch |
| `sectionDrillDown` | boolean | true when the chart has sections |
| `initialView` | `'fit'`, section label, or bounds | `'fit'` |
| `maxDpr` | number | 2 touch, 3 otherwise |
| `reducedMotion` | `'auto'`, `true`, `false` | `'auto'` |

### 3.4 Interface

| Option | Type | Default |
|---|---|---|
| `ui.zoomControls` | boolean | true |
| `ui.floorSelector` | boolean | true when more than one floor |
| `ui.legend` | boolean | true |
| `ui.filters` | `{ categories?, price?, accessible? }` or false | false |
| `ui.tooltip` | boolean or `(obj) => HTMLElement \| string` | true |
| `ui.holdTimer` | boolean | false |
| `ui.listViewToggle` | boolean | true |
| `theme` | `'light'`, `'dark'`, `'auto'`, or token object | `'auto'` |
| `colorblind` | boolean | false |
| `locale` | BCP 47 tag | `navigator.language` |
| `messages` | Partial message catalogue | built-in |
| `dir` | `'ltr'`, `'rtl'`, `'auto'` | `'auto'` |

### 3.5 Loading

| Option | Type | Default | Notes |
|---|---|---|---|
| `assets.baseUrl` | string | Package location | Where the WASM and atlas are served from |
| `assets.wasm` | `Response`, `ArrayBuffer`, or URL | — | Bring your own fetch |
| `fallback` | `'auto'`, `'canvas'`, `'list'` | `'auto'` | Force a step of the capability ladder |
| `worker` | boolean | false | P2 |

## 4. State adapters

The renderer does not care where seat state comes from. An adapter supplies it.

```ts
interface StateAdapter {
  /** Called once. Resolve with the first snapshot. */
  connect(ctx: AdapterContext): Promise<Snapshot>;
  /** Called after connect. Push deltas and private-hold updates through ctx. */
  subscribe?(): void;
  /** Hold-on-select. Omit for session: 'manual'. */
  hold?(labels: string[]): Promise<HoldResult>;
  release?(labels: string[]): Promise<void>;
  bestAvailable?(req: BestAvailableRequest): Promise<HoldResult>;
  disconnect(): void;
}

type Snapshot =
  | { kind: 'binary'; seq: number; bytes: ArrayBuffer }            // server wire format
  | { kind: 'map'; unavailable: string[]; mine?: string[] };       // labels; simplest for BYO

interface AdapterContext {
  chart: ChartInfo;
  applyDelta(d: { unavailable?: string[]; available?: string[] } | ArrayBuffer): void;
  setMine(labels: string[], expiresAt?: number): void;
  requestResync(): void;
  reportError(e: CaveaError): void;
}
```

Built-in adapters:

| Adapter | Use |
|---|---|
| `caveaServer(options)` | Talks to `cavea-server`: binary snapshot over HTTP, deltas over WebSocket with SSE and polling fallback, holds over HTTPS |
| `staticState({ unavailable })` | Fixed state, for static and spotlight modes or demos |
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

| Callback | Fires when | Payload |
|---|---|---|
| `onReady` | Chart-ready | Chart info, backend used, timing marks |
| `onSelectionChange` | The selection changes | `{ objects, holdToken, valid, expiresAt }` |
| `onObjectSelect` / `onObjectDeselect` | One object is added or removed | Object |
| `onSelectionInvalid` / `onSelectionValid` | Rule state changes | Rule, objects |
| `onHoldPending` / `onHoldSettled` | Hold requests start and finish | — (use to disable the checkout button) |
| `onHoldFailed` | A hold was refused | Objects with reasons |
| `onObjectUnavailable` | A selected object was taken | Object |
| `onHoldExpiring` | 60 s before expiry | Seconds left |
| `onHoldExpired` | The hold token expired | — |
| `onObjectHover` / `onObjectFocus` | Pointer or keyboard focus | Object or null |
| `onSectionEnter` | The camera drills into a section | Section |
| `onFloorChange` | The floor changes | Floor |
| `onCameraChange` | Pan or zoom settles | Centre, zoom, level |
| `onMetrics` | At most once a second | Frame statistics |
| `onError` | Any recoverable or fatal error | `CaveaError` |

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

## 6. Methods

| Method | Notes |
|---|---|
| `map.getSelection()` | Current objects and hold token |
| `map.select(labels)`, `map.deselect(labels)`, `map.clearSelection()` | Subject to the same rules as taps |
| `map.selectBestAvailable({ quantity, categories?, accessible? })` | Uses the adapter's `bestAvailable` |
| `map.setPricing(p)`, `map.setFilter(f)`, `map.setTheme(t)`, `map.setLocale(l)` | Live updates |
| `map.setMode(m)`, `map.setSpotlight(labels)` | |
| `map.zoomTo(target, opts?)` | Section label, object label, floor, bounds, or `'fit'` |
| `map.zoomIn()`, `map.zoomOut()` | |
| `map.setFloor(id)` | |
| `map.findObject(label)` | Details and current status |
| `map.listSections()`, `map.listCategories()` | With availability counts |
| `map.showListView(bool)` | Switch between map and accessible list |
| `map.resync()` | Force a fresh snapshot |
| `map.toImage({ width, height })` | PNG blob of the current view (thumbnails, confirmation pages) |
| `map.destroy()` | Releases the GL context, sockets and listeners. Does not release holds |

`createSeatMap` returns a promise that resolves at chart-ready and rejects only if no step of the capability ladder works.

## 7. Built-in interface

All built-in controls live in a shadow root on the container, so host styles cannot break them and theirs cannot leak.

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
interface CaveaError {
  code: string;        // stable: 'chart_fetch_failed', 'webgl_unavailable', 'state_disconnected', …
  fatal: boolean;
  message: string;
  cause?: unknown;
}
```

| Situation | Behaviour |
|---|---|
| Chart fetch fails | Retry three times with backoff, then show the error state with a retry action |
| WASM blocked or fails to compile | Fall to the list view if the adapter can supply sections and rows; otherwise fatal |
| WebGL2 unavailable | Canvas 2D fallback |
| GL context lost | Pause, restore, re-upload; no user-visible error if restored within 5 s |
| Realtime connection drops | Keep the map usable, show a discreet "reconnecting" indicator, resync on return; holds still go over HTTPS |
| Sequence gap | Automatic resync |
| Hold request fails for network reasons | Retry once, then revert the selection and report `onHoldFailed` |
| Unknown option | Console warning in development builds only |

After `onReady`, the renderer never throws into host code.

## 10. Loading and performance guidance

For integrators; the docs site repeats this.

- **Preload.** Add `<link rel="preload" as="fetch" crossorigin>` for the WASM and the chart, or call `preloadSeatMap(options)` as early as the route is known.
- **Self-host assets** on the same origin or a CDN with long cache lifetimes; all files are content-hashed.
- **Content Security Policy.** WASM compilation needs `script-src 'wasm-unsafe-eval'`. The renderer uses no `eval` and no inline scripts. Realtime needs the server's origin in `connect-src`.
- **Server-side rendering.** Importing the package on the server is safe; nothing touches `window` until `createSeatMap` runs.
- **Bundlers.** The WASM is referenced with `new URL('./cavea.wasm', import.meta.url)`, which Vite, webpack 5, Rollup and esbuild resolve.
- **Size the container** before creating the map. The renderer observes later resizes.
- **Do not hide the container** with `display: none` during creation; a zero-size canvas delays the first frame.

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
