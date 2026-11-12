# Accessibility

What the seat map does today for buyers who use a keyboard, a screen reader or system display settings, and what is not built yet. By the end you will know what to tell an accessibility reviewer, and what your page must add.

> **Not yet** A screen-reader user on a phone or tablet without a keyboard cannot choose seats on the map today. TalkBack and VoiceOver on iOS move by touch and swipe, and the map's seats are reached with arrow keys. They find the map, hear its name and instructions, and can use its buttons; with a connected keyboard the arrows work. The path for them is a list view, which is planned and not built. If you have accessibility obligations, offer another way to buy (a phone line, or best-available from your backend) and say so next to the map.

## Keyboard

The map is one stop in the page's Tab order, before its own buttons. Inside it, a focus moves from seat to seat:

| Key | What it does |
|---|---|
| Tab, Shift+Tab | Enter or leave the map; then the zoom buttons, the legend and the notices |
| → ← ↓ ↑ | Move to the next seat in that direction: along the row, across an aisle, to the nearest seat of the next row. The first arrow focuses the seat nearest the centre |
| Enter, Space | Select or deselect the focused seat, by the same rules as a tap |
| Escape | Hide the focus; the next arrow resumes from it |
| + (or =), − | Zoom in or out about the focused seat |
| 0 | Show the whole map |

The map follows the focus, zooming in far enough to see the seat. The focus ring is drawn on the canvas around the seat, with a halo in the background colour so it shows over any category colour. The map takes these keys only while it has focus, and never a key with Ctrl, Alt or Meta, so the browser's and your page's shortcuts keep working.

Not built yet: moving between sections when zoomed out, Home and End, Page Up and Page Down, A for the next available seat, T to hear the time left on the hold, and ? for help.

## Screen readers

- **The map** is announced as a "seat map" application, with the name from the `mapLabel` message (default "Seat map": override it to name your venue and event, [Messages](theming.md#messages-and-languages)) and the usage instructions from `mapHelp`.
- **The focused seat** is announced as the focus moves: "Orchestra, Row C, Seat 12. VIP, £95.00. Available. Aisle seat. 12 of 30 in the row." Unavailable seats say so. The parts come from the message catalogue, so a translation covers them.
- **The selection** is announced in a polite live region: "Orchestra, Row C, Seat 12 selected. 2 seats selected."
- **Refusals, a seat taken, a hold lost or expired, and a dropped connection** are the notices' text, in a polite region of their own.
- **The buttons** are native buttons and a native disclosure, named from the catalogue, with a visible focus outline.

The tooltip is visual; its content reaches screen readers through the focused seat's name. Not built yet: an assertive region for refusals and expiry, and names for every object kind and state (a seat held by the buyer is said as "Selected").

## Seeing the map

- **State without colour.** Available seats are solid, selected seats carry a tick, unavailable seats are hollow and crossed; the legend shows the same shapes.
- **Contrast.** The built-in themes meet WCAG 2.2 AA for the controls ([Theming](theming.md#colour-tokens)); `high-contrast` is a theme of its own. Category colours are the chart's: the designer warns when two are too alike.
- **Text size.** The controls' text follows the buyer's text size, at least 14 px.
- **Targets.** Buttons are at least 44 × 44 px. A tap counts within 12 px of a seat's centre, and a seat too small to tap reliably makes the map zoom in instead of selecting.
- **Reduced motion.** `reducedMotion` (default: the buyer's system setting) turns off the camera's glides and momentum, the pulsing on the canvas, the loading skeleton's pulse and the notices' slide.
- **Forced colours** (Windows high contrast): the controls take the system's colours; the canvas does not yet.

## Time limits

A hold expires (15 minutes by default, the event's `hold_ttl_seconds`), which makes choosing seats a timed task. `onHoldExpiring` fires 60 s before the end; the map shows no countdown and offers no "more time" itself yet. Tell the buyer how long their seats are held when the first one is, and give them a way to ask for more time: your backend can extend a hold token ([extend a hold token](../api/seats.md#extendHoldToken)).
