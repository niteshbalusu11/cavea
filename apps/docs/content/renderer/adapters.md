# State adapters

A state adapter tells the seat map which seats are unavailable and holds seats when the buyer selects them. This page describes the three the package has: `caveaServer` for a Cavea server, `staticState` for a fixed list, and `customState` for your own backend, with the interface each one implements.

| Adapter | Use it when |
|---|---|
| [`caveaServer`](#caveaServer) | Seat state and holds are on your Cavea server |
| [`staticState`](#staticState) | Nothing changes while the page is open: a venue preview, a demo |
| [`customState`](#customState) | Your own backend keeps the seat state, and you use only the seat map |

## `caveaServer`

<!-- declaration: caveaServer -->

<!-- members: CaveaServerOptions -->

An invalid option throws a fatal `CaveaError` (`invalid_option`, with `option` such as `state.event`) from `caveaServer` itself.

**At start** it fetches the event, its chart and its seat state with the publishable key, and opens a live connection that keeps the state current as other buyers hold and book. The chart is the version the event is on, so you give the map no `chart`.

### Sessions

`session` decides who makes the [hold token](../concepts/holds.md) and whether a tap holds:

| `session` | Hold token | What a tap does |
|---|---|---|
| `start` (default) | Made by the map at the buyer's first tap (not on load, so a visitor who never taps costs nothing), and kept in the tab's `sessionStorage` | Holds the seat |
| `continue` | `holdToken`, made by your backend and passed to the page | Holds the seat under your token |
| `manual` | None | Selects without holding: your page holds at checkout, from `getSelection()` |
| `none` | None | As `manual` |

With `start`, a reload of the tab brings the buyer's held seats back, selected, with one `onSelectionChange` carrying the token. The token is stored per tab, server, workspace and event; where `sessionStorage` cannot be used (a private window, a sandboxed frame) it lasts as long as the page. With `continue`, the token is checked at start and never stored: once it has ended (expired, or booked), taps fail and `state_failed` is reported once.

When an event has `require_session`, `start` and `continue` need a `sessionToken`, signed by your backend ([session tokens](../concepts/session-tokens.md)). A function is called at the first hold, and once more when the server says the token has expired:

```ts check=types
import { caveaServer } from "{{renderer_package}}";

const state = caveaServer({
  baseUrl: "https://seats.example.com",
  publishableKey: "pk_paste-your-publishable-key-here",
  event: "opening-night",
  sessionToken: async () => {
    const reply = await fetch("/api/seat-session", { method: "POST" });
    if (!reply.ok) throw new Error(`session token: ${reply.status}`);
    return ((await reply.json()) as { token: string }).token;
  },
});
```

### Holding

A tap is held at once on the server, one request at a time; the engine combines quick taps. How the server's answers reach your page:

| The server answers | The map |
|---|---|
| Held | Keeps the seat selected |
| `409 object_not_free` (someone was faster) | Drops the seat: `onObjectUnavailable`, `onHoldFailed` with reason `not_free`, a notice |
| `422 hold_limit_exceeded` | Drops the seat: `onSelectionInvalid` with that rule, `onHoldFailed`, a notice |
| `404`, the hold token is gone | Drops the whole selection (`onHoldLost`); the next tap makes a new token |
| `401`, the session token expired, with a `sessionToken` function | Asks for a new session token and tries again once |
| `429` or `503` | Tries again once if the wait the server asks for is at most 2 s; otherwise drops the seat (`hold_failed`) |
| No answer (network) | Tries again once after 500 ms; then drops the seat (`hold_failed`) |

Each request is cancelled after 8 s, and a hold gives up 18 s after it started. A release is tried the same way; releasing with a token that has ended counts as done.

### The live connection

`transport` is `auto` by default: a WebSocket, falling back to Server-Sent Events, then to polling every 2 s, when the better one fails twice without ever connecting. One that has worked is kept for the page's life and reconnected however often it drops, with a growing wait (from 0.5 s, at most 30 s); the map stays usable meanwhile and holds still work, and the first drop is reported (`state_disconnected`, a "Reconnecting…" notice). A connection resumes where it left off; a gap is filled with a fresh snapshot, without the page noticing.

With `pauseWhenHidden` (the default), a page hidden for 10 s drops its connection and reconnects at once when it is shown again. Hiding the page never releases holds.

On the WebSocket, the server sends the buyer's own holds and their expiry over the same connection. Over SSE and polling the map asks for them after each hold and release and once a minute.

The connection stops for good, reported as `state_disconnected`, when the event is archived or moves to another chart version (`state_failed`: reload the page to see the new chart).

## `staticState`

<!-- declaration: staticState -->

Every seat available except the labels you list. Nothing is held, and nothing changes after start: taps select at once.

```ts check=types
import { createSeatMap, staticState } from "{{renderer_package}}";
#hide declare const container: HTMLElement;

await createSeatMap(container, {
  chart: { url: "/charts/grand-hall.ccf" },
  state: staticState({ unavailable: ["Stalls-A-1", "Stalls-A-2"] }),
});
```

## `customState`

<!-- declaration: customState -->

Your own implementation of the interface below: `connect` resolves with the first state and keeps it current through the context it is given; `hold` and `release`, if you implement them, are called when the buyer selects and deselects. Without them, taps select at once and nothing is held. The [bring your own backend guide](../guides/own-backend.md) builds one step by step.

<!-- members: StateAdapter -->

**What the map guarantees.** `chart` (when you gave no `chart` option) and `connect` are called once, at start, together. `hold` and `release` are called one at a time, in order, each after the previous promise settled. A `hold` that rejects or takes more than 20 s fails (`hold_failed`) and the seat is dropped. `disconnect` is called once, last, and nothing after it. A `connect` that rejects may be called again (the map retries a start that may succeed later: [Errors](errors.md#retries)), so it must leave nothing open when it fails.

### The context

<!-- members: AdapterContext -->

Calls made before the first state is applied are kept and applied in order after it.

### State by label

A snapshot or a delta names seats by their labels. A label your chart does not have is skipped and reported (`state_failed`).

<!-- declaration: Snapshot -->

<!-- declaration: LabelDelta -->

`mine` is the buyer's own held seats: the map keeps them selected and selects them again after a reload. A seat that leaves the set leaves the selection (`onHoldLost`); dropping the buyer's seats when their hold expires is how `onHoldExpired` fires.

### Holds

<!-- declaration: HoldResult -->

<!-- declaration: HoldRefusal -->

A refusal is all or nothing: no seat of the request is held. Seats `failed` names leave the selection with their reasons; seats it does not name are asked for again with the next hold. `code` `not_found` means the hold token is gone and drops the whole selection. Retrying is the adapter's choice: the map does not.

The `binary` form is the Cavea server's own wire format, which `caveaServer` uses; an adapter of your own uses labels.
