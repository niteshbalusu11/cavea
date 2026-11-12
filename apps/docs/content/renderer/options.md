# Options

Every option `createSeatMap` takes, with its type and default. All are optional: with none, the map shows a chart you give it with every seat available. Callbacks, also options, are on their own page: [Callbacks](callbacks.md).

The table is generated from the package's type declarations, so it always matches the version this site describes. Names with a dot are inside an object: `selection.max` is `{ selection: { max } }`.

<!-- members: SeatMapOptions options -->

`createSeatMap` checks the options before it fetches anything. An invalid value rejects it with a `CaveaError` whose `code` is `invalid_option` and whose `option` names the option, such as `selection.max`. A [state adapter](adapters.md) must have `connect` and `disconnect`, and `hold` and `release` together or neither. In a development build an unknown option is warned about and ignored.

## Where the chart comes from

With `caveaServer` you give no `chart`: the adapter fetches the version of the chart the event is on. With your own backend or `staticState`, give the compiled chart, a `.ccf` file, by URL or as bytes:

<!-- declaration: ChartSource -->

## Selection

`selection.max` is the most seats a buyer may select, 1 to 100. A tap beyond it is refused: the seat is not selected, `onSelectionInvalid` fires with the rule `max`, and the built-in interface says why. `selection.min` is a readiness rule, not a refusal: below it the selection's `valid` is `false`, which is the signal to keep your checkout button disabled.

The server has its own limit: a hold token made from a web page holds at most the event's `max_objects` seats, 10 by default ([event settings](../concepts/events.md)). Keep `selection.max` at or under it, so that the map refuses the extra seat rather than the server (`hold_limit_exceeded`).

## Prices and ticket types

Prices are for display only: Cavea never charges. Give each [category](../concepts/charts.md) a price, or a list of ticket types, in minor units (pence, cents):

```ts check=types
import { createSeatMap, staticState } from "{{renderer_package}}";
#hide declare const container: HTMLElement;

const map = await createSeatMap(container, {
  chart: { url: "/charts/grand-hall.ccf" },
  state: staticState(),
  currency: "GBP",
  locale: "en-GB",
  pricing: [
    { category: "stalls", price: 4500 },
    {
      category: "circle",
      ticketTypes: [
        { key: "adult", label: "Adult", price: 3500 },
        { key: "child", label: "Child", price: 2000 },
      ],
    },
  ],
});
```

The legend shows each category's price, or the range of its ticket types (£20.00 – £35.00); the tooltip shows the price of the seat under the pointer. A selected seat whose category has ticket types gets the first type; the buyer can choose another in the tooltip, and `ticketType` and `price` on the [selected objects](callbacks.md#the-object-a-callback-reports) say which. `map.setPricing` replaces the prices of a live map.

An entry without a category, or without a price or ticket types, or with a price that is not a whole number from 0, is left out, with a warning in a development build. Without `currency`, a price is divided by 100 and shown with two decimals; with it, the currency's own minor unit divides it (`JPY` none, `GBP` two, `KWD` three). `formatPrice` replaces the formatting; whatever it returns is shown as text, never as markup.

<!-- declaration: CategoryPricing -->

<!-- declaration: TicketTypePrice -->

<!-- declaration: FormatPrice -->

<!-- declaration: PriceContext -->

## The first view and gestures

`initialView` is where the camera starts: `"fit"` (the whole chart), a section's label, or a rectangle in chart units. An unknown section is reported through `onError` (`invalid_input`) and the chart is fitted.

<!-- declaration: Bounds -->

`gestureHandling` decides which gestures move the map. `greedy`, the default, lets one finger or the wheel pan and zoom: right for a page that is mostly the map. `cooperative` leaves one-finger scrolling and the plain wheel to the page and moves the map with two fingers or Ctrl and the wheel: right for a map in the middle of a long page. `none` leaves every gesture to the page.

<!-- declaration: GestureHandling -->

## The built-in interface

The map draws its own controls over the canvas: zoom buttons, a legend, a tooltip, short notices (toasts), and a loading and error state. Each is on unless its `ui` option is `false`; one that is off is not created, and its callbacks still fire, so you can build your own. A function for `ui.tooltip` replaces the tooltip's content with yours, in the same positioned box:

```ts check=types
import { createSeatMap, staticState } from "{{renderer_package}}";
#hide declare const container: HTMLElement;

await createSeatMap(container, {
  chart: { url: "/charts/grand-hall.ccf" },
  state: staticState(),
  ui: {
    tooltip: (seat) => `${seat.label}: ${seat.status === "unavailable" ? "taken" : seat.category.label}`,
  },
});
```

A string is shown as text. An element is shown as it is: if you build it from untrusted strings with `innerHTML`, that is your page's risk. A function that throws is reported as `callback_failed` and nothing is shown.

<!-- declaration: TooltipRenderer -->

<!-- declaration: TooltipObject -->

The controls live in a shadow root, so your page's CSS cannot break them; style them with [theme tokens and parts](theming.md). Without `ui.loading`, a chart that cannot be fetched rejects `createSeatMap` instead of showing the error state with its retry button ([Errors](errors.md#what-the-buyer-sees)).

## Not built yet

These are planned and not in the type. Passing one warns in a development build and is otherwise ignored:

- `floor` (only a chart's first floor is drawn), `ui.floorSelector`, `onFloorChange`, `onSectionEnter`, `sectionDrillDown`;
- `selection.groupSize`, `selection.validators` (such as no single empty seat), `selection.categories`, `selection.selectable`, `selection.restore` (holds are always restored after a reload);
- `ui.filters`, `ui.listViewToggle`, `colorblind`, `fallback` (a Canvas 2D or list fallback), `worker`, `zoomToSelect` (seats too small to tap are always zoomed to);
- `ui.holdTimer`, a countdown: the data for your own is there, `expiresAt` on the selection and `onHoldExpiring`;
- the `static`, `spotlight` and `manager` modes: `mode` takes only `picker`, and another value runs as `picker`.
