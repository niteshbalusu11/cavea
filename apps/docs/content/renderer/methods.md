# Methods

What your page can ask of a map once `createSeatMap` has resolved. Methods never throw: input the map refuses (an unknown label, an invalid theme) is reported through `onError` as `invalid_input`. After `destroy` they do nothing, with a warning in a development build.

<!-- members: SeatMap -->

## Selecting from your page

`select`, `deselect` and `clearSelection` follow the same rules as the buyer's taps: `selection.max`, the holds, and the callbacks. `select` is all or nothing: if one label is unknown or refused, none is selected. Use it to restore a basket the buyer built elsewhere, or for your own "best seats" button.

```ts check=types
import type { SeatMap } from "{{renderer_package}}";
#hide declare const map: SeatMap;

const seat = map.findObject("Stalls-C-12");
if (seat !== null && seat.status === "available") {
  map.select([seat.label]);
  map.zoomTo(seat.label);
}
```

## Moving the camera

`zoomTo` takes `"fit"`, a section's label, a seat's label or a rectangle in chart units; a string names a section if one has that label, else a seat. It is animated unless you pass `{ animate: false }`. The camera stays within the chart, so a seat near its edge may not end up centred.

## Sections and categories

`listSections()` counts each section's seats and the ones not drawn unavailable: the free ones and the buyer's own. `listCategories()` lists the chart's categories with their colours (as `#rrggbbaa`) and bookable seats. Availability by category is not counted yet.

## Destroying a map

`destroy()` disconnects the state adapter, removes every listener, timer and pending frame, frees the WebAssembly view and removes the canvas. It does not release the buyer's holds: they stay until they expire or your backend books or releases them. A second `destroy` does nothing. Aborting the `signal` option does the same, and also cancels a map that is still loading.

## Not built yet

`selectBestAvailable`, `setFilter`, `setMode`, `setSpotlight`, `setFloor`, `showListView` and `toImage` are planned. They are not in the type; called anyway, they do nothing and warn in a development build.
