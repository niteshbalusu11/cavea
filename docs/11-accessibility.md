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

### 3.1 The baseline (A11Y-001, built)

The M1 baseline is the keys below; the rest of the table, focus between sections at overview and the two-tone ring's polish are `A11Y-010` and `A11Y-011`.

| Key | Baseline behaviour |
|---|---|
| Tab / Shift+Tab | The canvas is the map's one tab stop (`tabindex="0"`), before the built-in controls. Arriving by keyboard (`:focus-visible`) shows the remembered focus, if there is one; a click on the map focuses it without showing a ring |
| → ← ↓ ↑ | Move the virtual focus (§3.2). With no focus yet, the first arrow focuses the seat nearest the centre of the view |
| Enter, Space | Select or deselect the focused seat, as a tap (`04-renderer.md` §16: the same rules, refusals and holds), but never zoom-to-select |
| Escape | Hide the focus (it is remembered: the next arrow resumes from it). Taken only while a focus is shown, so the page's own Escape still works otherwise |
| + (or =), − | Zoom in or out one step about the focused seat, else the centre |
| 0 | Fit the whole chart |

- Only these keys are taken from the page (their default is prevented), and only while the map has the page's focus; a key with Ctrl, Alt or Meta is never taken, so the browser's and the host's shortcuts work. A key during an input method's composition is ignored.
- **The focus is always a seat.** At overview and in the cross-fade, where seats are not drawn, an arrow still moves between seats and the camera zooms to the detail level (14 px seats) on the focused one; Enter selects there. Moving focus between sections at overview, with Enter to open one, is `A11Y-010`, with the section name pattern of §5.
- **The camera follows** with the smallest movement: below the seats level it zooms to 14 px seats centred on the focus; otherwise it pans just enough to keep the seat's centre 32 CSS px inside the view (`tuning::FOCUS_MARGIN_PX`, at most a quarter of the view), animated unless motion is reduced. A focused seat under a built-in control (the zoom controls, the legend) is then moved, once per seat, the least distance that clears the control by 16 px.
- **The ring** is the engine's existing focus ring (`04-renderer.md` §7, §19): a halo in the background colour and a 2 px ring in `ring-focus` outside the seat, drawn in the `shape` program as overlay instances (no new draw call), with 4.9, 6.9 and 19.6 to 1 against the background in light, dark and high contrast. The map's own focus, before any seat is focused, is a 2 px `ring-focus` outline inside the canvas.
- **Memory.** The engine keeps the focused seat while its ring is hidden: tabbing away and back, or Escape, returns to it. A tap moves the remembered focus to the tapped seat without showing it. A seat becoming unavailable keeps the focus (its name says so); a new chart forgets it.

### 3.2 Which seat an arrow moves to

In screen directions (the camera has no rotation, so these are the chart's directions, y down), from the focused seat `f`:

1. **Along the row.** `f`'s neighbour in its row (the seat before or after it in `idx` order, across a gap between segments) that lies within 45° of the arrow's direction.
2. **In the cone.** Otherwise, of the seats of the floor's other rows within 45° of the direction, the one with the lowest `along + 2 × |across|`: `along` its distance in the arrow's direction, `across` its offset perpendicular to it.
3. **In the half-plane.** Otherwise, the same over the seats of other rows strictly ahead (`along > 0`).
4. Otherwise `f` is at the edge in that direction: the focus stays and nothing is said.

Equal scores go to the lower `idx`, so the result depends only on the chart, the floor shown and `f`. So → and ← walk a row, curved or angled up to 45° (a steeper row is walked with ↑ and ↓, the arrows nearer its direction), and step across an aisle or a section gap to the nearest seat level with the row; ↓ and ↑ move to the nearest seat of the next row in that direction, "away from or towards the stage as drawn" when the stage is above or below. Tested as properties (`crates/cavea-render/src/focus/tests.rs`): against a brute-force statement of the rule on `theatre`, `arena` and synthetic stadiums; every result strictly ahead, an edge only when no other row's seat is ahead; → then ← returns along a straight row; every seat of every section of `theatre`, `arena` and a synthetic stadium reaches, and is reached from, every other. The search uses the hit test's hierarchy (`04-renderer.md` §15): measured locally (release build, AMD Ryzen 9 7900X), 1.2 µs a search on `stadium-100k` and 2.1 µs for a whole arrow key through the engine (search, camera, event) at overview and at detail.

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

Today (05 §7): the zoom controls, legend, tooltip, toasts and the loading and error states are native buttons and a `<details>` disclosure, named from the message catalogue, reachable with Tab after the canvas, with a visible focus outline. The tooltip is visual: its content reaches screen readers through the proxy's name, not the tooltip. On touch, a long press shows it and a tap that selects pins it to the seat.

**The baseline (A11Y-002, built).**

- **The map** is the canvas: `role="application"`, `aria-roledescription` from `mapRole` ("seat map"), `aria-label` from `mapLabel` ("Seat map": the host names the venue and event by overriding it in `messages`, since the chart knows only its own name), `aria-describedby` pointing to the usage instructions (`mapHelp`: "Use the arrow keys to move between seats, and Enter to choose one.").
- **The proxy** is in the canvas's fallback content, which browsers expose to assistive technology and never draw. While a focus is shown, the canvas's `aria-activedescendant` names it. It is `role="button"` (Enter activates the seat, and screen readers say a button's name and role on an active-descendant change in application mode), with its name as text. There are two, written alternately, so every new name also changes `aria-activedescendant`, which is what NVDA, JAWS and VoiceOver announce; a name set again on the same element is not reliably announced. Only the active one is in the tree (the other is `hidden`), so browse mode never finds a stale name. The name is rebuilt only when the engine reports the focus (it moved, appeared, or its seat's state changed), and is skipped when it is unchanged; nothing runs per frame.
- **Name patterns built:** the seat and unavailable-seat patterns above, from the same data as the tooltip (`findObject`: the selection's ticket type and the host's prices) and the engine's place of the seat in its row: "Orchestra, Row C, Seat 12. VIP, £95.00. Available. Aisle seat. 12 of 30 in the row." and "Orchestra, Row C, Seat 14. Unavailable." The parts come from the catalogue (`row`, `seat`, `available`, `selected`, `unavailable`, the four flags, `place`), so `messages` localises them. "Held by you" is said as Selected until `A11Y-012` adds every state; the section pattern waits for focus at overview (`A11Y-010`).
- **One polite live region** (`role="status"`, visually hidden, in the shadow root) says the buyer's selection changes: "Orchestra, Row C, Seat 12 selected. 2 seats selected." (`seatSelected`, `seatDeselected`, `selectedCount`), once per batch of engine events. Several seats at once (a host's `select`) say only the count. The same words twice are varied by an invisible no-break space, so they are heard again.
- **Which region says what.** Refusals (max, not free, hold failures), a seat taken, a hold lost or expired, and the reconnecting notice stay in the toasts' polite region (05 §7); the selection region does not say the removals those caused (a seat taken, lost, or refused and reverted, even when the revert arrives in a later batch), so each event is said once. The assertive region for refusals and expiry is `A11Y-013`; when it comes, those move out of the toasts' region.
- **Shadow root.** The canvas, its proxies and description, and the live region are in the map's shadow root, and every id they reference (`aria-activedescendant`, `aria-describedby`) is in the same root; nothing references across the boundary. Chromium's accessibility tree shows the application, its description and the active descendant's name, and the live region's text (§9.2).
- **Touch screen readers.** TalkBack and VoiceOver on iOS move by touch and swipe, not arrow keys. With this baseline they find the map, hear its name and instructions, and can use the built-in controls; with a connected keyboard the arrows work as on a desktop. They cannot move between seats by swiping, and a double tap on the map activates its centre point, which a screen-reader user cannot aim. The path for them is the list view (`A11Y-014`), which does not exist yet: until it does, the map is not usable by a touch screen-reader user without a keyboard.

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
| Target size | A touch target is at least 24 CSS px: the hit tolerance reaches 12 px from a seat's centre. Seats smaller than 18 CSS px trigger zoom-to-select (`04-renderer.md` §15) |
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

**The baseline's checks (A11Y-001, A11Y-002).** No screen reader was available when the baseline was built: the checks so far are Chromium's accessibility tree over the DevTools protocol and unit tests. A person checks, with NVDA (Firefox and Chrome), VoiceOver (Safari, macOS and iOS) and TalkBack (Chrome, Android), on the demo (`apps/demo`, `?preset=theatre&prices=1`):

1. Tabbing to the map says "Seat map, seat map" (or the application role) and the instructions; NVDA switches to focus mode by itself.
2. Each arrow says the new seat's name once (not the map again), in full; at an edge nothing is said.
3. Enter on an available seat says the selection ("… selected. 1 seat selected.") once, and the name, if the screen reader repeats it, says Selected; Enter on an unavailable seat says the toast's refusal once and nothing else.
4. A seat taken by someone else (`?snipe=1`, or `caveaDemo.take(label)`) is said once, by its toast.
5. Escape, then an arrow, resumes from the same seat; tabbing to the zoom controls and back returns to it.
6. VoiceOver's and NVDA's browse mode do not find stale seat names in the page.
7. Arrow keys still scroll the page when the map does not have focus; the screen reader's own shortcuts (NVDA+key, VO+key) are not taken by the map.
8. On iOS and Android: the map is found by touch and swipe and its name and instructions are said; what a double tap on it does is noted (it activates the centre point).

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
