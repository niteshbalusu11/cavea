# Callbacks

What the seat map tells your page, and the objects it reports. Callbacks are options of `createSeatMap`; all are optional.

<!-- members: SeatMapOptions callbacks -->

## How callbacks are called

- **Order.** In the order things happen. A method you call from inside a callback (say `map.deselect` in `onObjectSelect`) has its own callbacks delivered after the ones already being delivered.
- **Before ready.** A buyer can tap before `createSeatMap` resolves, while the first state is arriving, so selection callbacks can fire before it does. `onReady` fires just before it resolves.
- **Errors in your code.** A callback that throws is caught and reported through `onError` as `callback_failed`; it never breaks the map. If `onError` itself throws, that is ignored.
- **After `destroy`**, nothing is called.

## A checkout button

The usual wiring: enable checkout while there is a valid selection and no hold is in flight, and send the hold token to your backend.

```ts check=types
import { caveaServer, createSeatMap, type Selection } from "{{renderer_package}}";
#hide declare const container: HTMLElement;
#hide declare const checkout: HTMLButtonElement;

let selection: Selection | null = null;
let holding = false;
const update = (): void => {
  checkout.disabled = holding || selection === null || !selection.valid || selection.objects.length === 0;
};

await createSeatMap(container, {
  state: caveaServer({
    baseUrl: "https://seats.example.com",
    publishableKey: "pk_paste-your-publishable-key-here",
    event: "opening-night",
  }),
  selection: { min: 1, max: 6 },
  onSelectionChange: (s) => {
    selection = s;
    update();
  },
  onHoldPending: () => {
    holding = true;
    update();
  },
  onHoldSettled: () => {
    holding = false;
    update();
  },
  onHoldExpiring: (secondsLeft) => {
    console.log(`Your seats are held for ${secondsLeft} more seconds.`);
  },
  onError: (error) => {
    if (!error.fatal) console.warn(error.code, error.message);
  },
});
```

The [hold, pay, book guide](../guides/hold-pay-book.md) follows the hold token to the booking.

## Holds, losses and expiry

With [`caveaServer`](adapters.md#caveaServer) a seat is held on the server when the buyer selects it. What can happen to a selected seat after that:

| What happened | Callbacks | The buyer sees (built-in interface) |
|---|---|---|
| The hold was refused because someone else has the seat | The seat leaves the selection (`onObjectDeselect`, `onSelectionChange`), `onObjectUnavailable`, then `onHoldFailed` with reason `not_free` | "… was just taken by someone else." |
| The hold was refused for another reason, such as a limit | The seat leaves the selection; `onSelectionInvalid` with the refusal's code as its rule (`hold_limit_exceeded`, say), then `onHoldFailed` | A notice saying why |
| A selected seat became unavailable where selecting holds nothing (a manual session, an adapter without holds) | `onObjectUnavailable`; the seat leaves the selection | "… was just taken by someone else." |
| The buyer's hold ended early: released elsewhere, or the hold token is gone | `onHoldLost` with the seats; they leave the selection | "Your hold on … has ended." |
| The hold is about to expire | `onHoldExpiring` 60 s before, with the seconds left | Nothing: a countdown is not built yet |
| The hold expired | `onHoldLost`, then `onHoldExpired` | "Your hold expired and your seats were released." |

`onHoldExpiring` is timed by the server's clock, corrected for the buyer's, so a buyer whose clock is minutes off still gets it on time. It fires at once, with fewer seconds, when the expiry is less than 60 s away when the map learns of it (after a reload, say).

## The selection

<!-- members: Selection -->

`expiresAt` is on the page's clock (`Date.now()`), so the seconds left are `(selection.expiresAt - Date.now()) / 1000`.

## The object a callback reports

Every object a callback or method reports has this shape. Only seats exist today: `kind` is always `"seat"`.

<!-- members: SeatObject -->

`label` is the seat's full name in the chart, which your backend books by: section, row and seat joined as the chart's label template says (`Stalls-C-12`). A part that is empty is left out with its separator: a seat in no section is `C-12` with the default template.

`findObject` and the tooltip add the status:

<!-- declaration: ObjectStatus -->

## Ready and metrics

<!-- members: ReadyInfo -->

`marks` are milliseconds from `createSeatMap` to each step of the start: `init_start`, `wasm_ready`, `gpu_ready`, `chart_fetched`, `chart_decoded`, `first_frame` and `state_ready`. Each is also a `performance.mark` named `cavea:<name>`, so it shows in the browser's performance panel. `first_frame` is missing while the container is hidden.

With `onMetrics`, the map times its frames and reports, at most once a second while it draws:

<!-- members: Metrics -->

Without `onMetrics` the map reads no clock per frame. The times are your page's main-thread work for a frame, not the GPU's.
