# 11 · Accessibility

**Status:** Draft v0.1 · 2026-11-10

No competitor documents keyboard or screen-reader support for ticket buyers. Customers who sell in the EU have accessibility obligations for their checkout. An accessible seat picker is therefore both the right thing to build and a sales argument (`DECISIONS.md` D-16).

## 1. Standard and scope

| Surface | Target |
|---|---|
| Buyer renderer (map, built-in controls, list view) | WCAG 2.2 level AA |
| Console | WCAG 2.2 level AA |
| Designer | Keyboard-operable chrome; full screen-reader operation is out of scope for v1 |
| Native mobile SDKs | Platform accessibility guidelines; VoiceOver and TalkBack support |

We claim conformance only after the testing in §9, and publish an Accessibility Conformance Report after an external audit.

## 2. Principles

1. **The map is an enhancement.** Everything a buyer can do on the map can be done in the list view.
2. **Never colour alone.** Every state and category is distinguishable without colour.
3. **One tab stop.** The map is a single stop in the page's tab order, with its own internal navigation.
4. **Say what changed.** Selections, refusals and expiries are announced.
5. **Respect settings.** Reduced motion, contrast preferences, text size and forced colours are honoured.

## 3. Keyboard model for the map

The map takes focus as one element. Inside it, a **virtual focus** moves between seats.

| Key | Action |
|---|---|
| Tab / Shift+Tab | Enter or leave the map; then move through the built-in controls |
| → / ← | Next or previous seat in the row. At a gap, jump to the next segment |
| ↓ / ↑ | Nearest seat in the next row away from, or towards, the stage as drawn |
| Home / End | First or last seat of the row |
| Page Down / Page Up | Next or previous section |
| Enter or Space | Select or deselect the focused object; in overview, enter the focused section |
| A / Shift+A | Next or previous **available** seat |
| + / − | Zoom in or out |
| 0 | Fit the whole chart |
| Escape | Leave the section back to overview; a second press returns focus to the map container |
| L | Switch to the list view |
| T | Announce the time left on the hold |
| ? | Show the keyboard help |

Behaviour:

- In the overview level, focus moves between sections with the arrow keys.
- The camera follows the virtual focus and keeps the focused object in view.
- The focus ring is drawn on the canvas: 2 px, with at least 3:1 contrast against both the seat and the background (a two-tone ring).
- Focus is remembered when the buyer tabs away and returns.
- Keyboard selection obeys the same rules as pointer selection and never requires zooming first.

## 4. List view

A complete alternative to the map, built from ordinary HTML controls. It is offered by a visible control placed **before** the map in the page order: "Choose seats from a list".

```
Choose seats from a list
───────────────────────────────────────────────
Floor      [ Stalls ▾ ]      Category  [ Any ▾ ]
Quantity   [ 2 ▾ ]           ☐ Accessible seats only

[ Find the best 2 seats together ]

▾ Orchestra · from £45 · 212 available
    Row C   ☐ 12  ☐ 13  ☐ 14  ☐ 15  ☐ 16  ☐ 17      6 together
    Row D   ☐ 3   ☐ 4            ☐ 21  ☐ 22          2 + 2
▸ Balcony · from £30 · 96 available
▸ Standing · £25 · available          Quantity [ 1 ▾ ]

Selected: Orchestra C 12, C 13 · £90      Hold expires in 14 minutes
```

Requirements:

- Sections are headings with disclosure buttons. Rows are groups with a label; seats are checkboxes whose accessible name is the full description ("Row C, seat 12, £45, available, aisle seat").
- Only available seats are listed. Runs of adjacent seats are shown and described ("6 together").
- "Find the best *n* seats together" calls best-available.
- Filters match the map's filters; both views share one selection.
- Rows are loaded per section on expansion.
- General-admission areas use a quantity control.
- Selection changes, refusals and the hold timer are announced as on the map.
- **Two implementations:** one fed by the engine (normal case), and one fed by JSON from the adapter for environments where WASM or canvas is unavailable (`08-server.md` §6.2, public section and row endpoints).

## 5. Screen-reader model for the map

| Element | Markup |
|---|---|
| Map container | `role="application"`, `aria-roledescription="seat map"`, `aria-label` naming the venue and event, `aria-describedby` pointing to short usage instructions |
| Focused object | A visually hidden proxy element referenced by `aria-activedescendant`, with an accessible name rebuilt on every focus move |
| Announcements | One polite live region for selections and counts; one assertive live region for refusals and expiry |
| Built-in controls | Native buttons and form controls |

Proxy name pattern:

- Seat: "Orchestra, row C, seat 12. VIP, £95. Available. Aisle seat. Seat 12 of 30."
- Section in overview: "Orchestra. From £45. 212 seats available. Press Enter to open."
- Unavailable seat: "Orchestra, row C, seat 14. Unavailable."

Announcement patterns:

- "Seat C 12 selected. 2 seats selected."
- "Seat C 13 was just taken by someone else. It has been removed from your selection."
- "You cannot leave a single empty seat. Choose seat C 14 or C 16 instead."
- "Your seats are held for one more minute."

All strings come from the message catalogue and are localised. Prices use the host's formatter.

Because application mode changes how screen readers handle keys, the list view is the path we recommend to screen-reader users, and the instructions say so.

## 6. Visual requirements

| Requirement | Rule |
|---|---|
| State without colour | Available: solid shape. Unavailable: hollow or crossed. Selected: tick mark and ring. Held by you: tick and timer icon in the legend |
| Category without colour | Legend lists each category with its colour swatch, name and price; the tooltip and the proxy name state the category; an optional letter mark inside seats at detail level |
| Non-text contrast | Seats, rings and controls have at least 3:1 against adjacent colours |
| Text contrast | At least 4.5:1, including seat labels on every category colour (ink colour is chosen per palette entry) |
| Colour-blind safety | Built-in palettes are checked under simulated protanopia, deuteranopia and tritanopia |
| Target size | Selecting by touch requires seats of at least 24 CSS px; smaller seats trigger zoom-to-select |
| Text resize and reflow | Built-in controls reflow at 200% text size and at 320 px width |
| Forced colours | When `forced-colors` is active, the renderer switches to a theme built from system colours |
| Motion | With reduced motion requested: no inertia, no fly-to animation, no pulse |
| Flashing | Nothing flashes more than three times a second |

The designer warns when a chart's category colours are too similar or too weak (`03-chart-format.md` §3.2, W107).

## 7. Timing

Seat holds expire, which makes this a timed interaction.

- The buyer is told the hold duration when the first seat is held.
- At 60 seconds before expiry, `onHoldExpiring` fires and a dialog offers "I need more time".
- Extending is the host's decision (it calls its backend, which calls the extend endpoint). If the host does not allow extension, the dialog says so and explains what will happen.
- The remaining time is available on demand (a button and the `T` key), not announced continuously.
- After expiry, the selection is cleared, the buyer is told why, and the map remains usable.

## 8. Console and designer

- Console screens meet WCAG 2.2 AA: labelled controls, visible focus, keyboard-reachable actions, error identification, headings and landmarks, sufficient contrast in both themes.
- Tables support keyboard row navigation and expose sort state.
- The event manager offers the list-style object table as an alternative to map selection.
- The designer's chrome is keyboard-operable, and the main objects can be created from forms (`07-designer.md` §15).

## 9. Testing

### 9.1 Automated

- axe-core runs in Playwright against the built-in controls, the list view and every console screen. No violations at AA.
- Unit tests assert the proxy name and announcement for each state and each rule violation.
- A contrast test computes ratios for every palette entry and theme.
- A colour-blind simulation test checks that state pairs remain distinguishable.

### 9.2 Manual matrix (per release that touches the renderer)

| Screen reader | Browser | Platform |
|---|---|---|
| NVDA | Firefox, Chrome | Windows |
| JAWS | Chrome | Windows |
| VoiceOver | Safari | macOS |
| VoiceOver | Safari | iOS |
| TalkBack | Chrome | Android |

Also: keyboard only; 200% and 400% zoom; Windows High Contrast; reduced motion.

### 9.3 Journeys that must pass

For each assistive setup in the matrix:

1. Find and select two adjacent seats in a named section, using the list view.
2. Do the same on the map with the keyboard.
3. Use "best available" for four seats.
4. Select one wheelchair-accessible seat and a companion seat.
5. Lose a seat to another buyer and recover.
6. Reach the expiry warning and request more time.
7. Change floor, filter by category, and deselect a seat.
8. Buy two general-admission tickets.

### 9.4 People

- Before claiming conformance: sessions with at least three screen-reader users and one switch or keyboard-only user, on the demo checkout.
- Before publishing a conformance report: an external audit.

## 10. Deliverables

| Deliverable | When |
|---|---|
| Baseline keyboard navigation and proxy naming | M1 |
| Full keyboard model, live regions, non-colour cues | M6 |
| List view, both implementations | M6 |
| Accessibility statement and integration guide | M6 |
| Conformance report after audit | When revenue allows; listed in `14-go-to-market.md` |
