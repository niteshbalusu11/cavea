# Theming and text

How to make the seat map match your site: a built-in theme, colour tokens over it, CSS for the controls, and the words it shows in any language. By the end you will know which knob changes what.

## Themes

`theme` takes a built-in theme, `auto`, or tokens over a base:

<!-- declaration: Theme -->

<!-- declaration: ThemeName -->

`auto`, the default, is `light` or `dark` following the buyer's system setting, and changes with it. `high-contrast` is black and white with yellow for the selection. `map.setTheme` switches a live map; the canvas redraws without reloading the chart.

```ts check=types
import { createSeatMap, staticState } from "{{renderer_package}}";
#hide declare const container: HTMLElement;

const map = await createSeatMap(container, {
  chart: { url: "/charts/grand-hall.ccf" },
  state: staticState(),
  theme: { base: "light", tokens: { "seat-selected-fill": "#7c3aed", "category-stalls": "#0f766e" } },
});
map.setTheme("dark");
```

## Colour tokens

A token is a CSS custom property. Set it on the map's container or any ancestor, in your stylesheet, or pass it in `theme.tokens` (with or without `--cavea-`), which wins over the page's. Colours may be in any CSS syntax: `#hex`, `rgb()`, `oklch()`, names.

```css
#map {
  --cavea-seat-selected-fill: #7c3aed;
  --cavea-ring-focus: #7c3aed;
  --cavea-font: "Inter", system-ui, sans-serif;
}
```

| Token | What it colours |
|---|---|
| `--cavea-bg` | The canvas's background, and the controls' surface |
| `--cavea-fg` | Text, on the canvas and in the controls |
| `--cavea-fg-muted` | Muted text, and the controls' borders |
| `--cavea-seat-available-stroke` | The outline of an available seat; its fill is its category's colour |
| `--cavea-seat-unavailable-fill` | An unavailable seat's fill, the legend's unavailable shape, and the loading skeleton |
| `--cavea-seat-unavailable-stroke` | An unavailable seat's outline and cross |
| `--cavea-seat-selected-fill` | A selected seat, and the legend's selected shape |
| `--cavea-seat-selected-ink` | The tick on a selected seat |
| `--cavea-ring-hover` | The ring around the seat under the pointer |
| `--cavea-ring-focus` | The keyboard focus ring, on the canvas and on the controls |
| `--cavea-section-tint` | How strongly sections are tinted when zoomed out: a number from 0 to 1 |
| `--cavea-dim` | The opacity of objects a filter leaves out; filters are not built yet |
| `--cavea-font` | The font family of the controls and of text the canvas draws itself (section names, labels the built-in atlas cannot draw). Default: the page's font for the controls, the system font for the canvas. The map uses fonts the page already has and never fetches one |
| `--cavea-radius` | The controls' corner radius (default `8px`) |
| `--cavea-shadow` | The controls' shadow |
| `--cavea-category-<key>` | One category's colour, on the canvas and in the legend, over the chart's; `<key>` is the category's key |

An unknown token or a value the map cannot use is reported through `onError` (`invalid_input`) and the canvas keeps its colours.

The built-in themes' colours for the tokens the controls also use:

<!-- theme-defaults -->

The built-in themes meet WCAG 2.2 AA for the controls, checked by calculation in the package's tests: text at least 4.5:1 on its surface; borders, the focus ring and the legend's state shapes at least 3:1. Colours you set are yours to check.

### When the map notices a change

CSS has no event for a custom property changing, so the map reads the tokens again when you call `setTheme`, when the system's light or dark setting changes, and when an attribute of the container, `<html>` or `<body>` changes (a class or `data-theme` toggle, the usual way sites switch themes). After any other change, such as a stylesheet swapped in, call `map.refreshTheme()`.

## Styling the controls

The controls are in a shadow root, so your CSS does not reach inside by accident. Style them on purpose with `::part()`:

```css
#map::part(legend) {
  font-size: 0.8125rem;
}
#map::part(button) {
  border-radius: 999px;
}
```

| Part | Element |
|---|---|
| `zoom-controls`, `zoom-in`, `zoom-out`, `zoom-fit` | The zoom buttons and their column |
| `legend`, `legend-summary`, `legend-categories`, `legend-states` | The legend (a disclosure), its summary and its two lists |
| `tooltip`, `ticket-types`, `ticket-type` | The tooltip, and the ticket-type choice inside it |
| `toasts`, `toast`, `toast-dismiss` | The notices, each notice, and its close button |
| `loading`, `error`, `retry` | The loading skeleton, the error state and its retry button |
| `button` | Every button |

Text in the controls is at least 14 px, and follows the buyer's text size.

## Messages and languages

Every word the map shows comes from one English catalogue. `messages` replaces any of them by id; `locale` sets the language of plural forms, numbers and prices (default: the browser's); `dir` the controls' direction (default: the locale's script, else the page's). `map.setLocale` changes the language of a live map.

```ts check=types
import { createSeatMap, staticState } from "{{renderer_package}}";
#hide declare const container: HTMLElement;

await createSeatMap(container, {
  chart: { url: "/charts/grand-hall.ccf" },
  state: staticState(),
  locale: "fr-FR",
  messages: {
    mapLabel: "Plan de salle : Grand Hall, soirée d'ouverture",
    zoomIn: "Zoom avant",
    max: { one: "Un seul siège.", other: "{count} sièges au plus." },
  },
});
```

`{name}` in a message is a value the map fills in; numbers are formatted for the locale. A message with a count is an object of plural forms (`one`, `few`, `many`, `other`, …), chosen by the locale's rules, falling back to `other`. An unknown id is ignored (with a development warning); a message that is not a string or an object with `other` is `invalid_option`. Override `mapLabel` to name your venue and event: it is what a screen reader says first.

<!-- declaration: Messages -->

<!-- messages -->
