# 04 · Renderer

**Status:** Draft v0.1 · 2026-11-10

The renderer draws a chart and its live seat state, handles pan, zoom and selection, and reports what the buyer did. It is one Rust engine with a small host layer per platform.

Budgets are in `01-architecture.md` §5. The public JavaScript API is in `05-web-sdk.md`.

## 1. Goals

1. First interactive frame as early as the network allows.
2. 60 fps on the reference Android phone at 100,000 seats; 120 fps on capable displays.
3. Zero work when nothing changes.
4. The same engine on web, iOS and Android, and inside the designer.
5. A buyer can always complete a purchase, whatever their device supports.

Not goals: 3D, arbitrary vector graphics, effects.

## 2. Layering

```
┌─────────────────────────────────────────────────────────┐
│ Host (per platform)                                     │
│  surface / GL context · input events · clock · frame    │
│  scheduling · text rasteriser · image decoding · a11y   │
└───────────────▲───────────────────────┬─────────────────┘
        events  │                       │ commands
┌───────────────┴───────────────────────▼─────────────────┐
│ cavea-render: Engine                                    │
│  scene · camera · gestures · LOD · selection · draw list│
└───────────────┬─────────────────────────────────────────┘
                │ GpuBackend trait
      ┌─────────┴─────────┬──────────────────┐
      │ glow (WebGL2,     │ wgpu (Metal,     │ Canvas 2D (web
      │ GLES3)            │ Vulkan, GLES)    │ fallback, in TS)
      └───────────────────┴──────────────────┘
```

The engine performs no I/O itself. The host supplies five services:

- **Clock and frame scheduler: through the protocol, not traits.** The host's clock stamps every timed command (`Pointer`, `Wheel`, `Tick`); the engine asks for a frame with the `NeedFrame` event and the host's scheduler (`requestAnimationFrame`, a display link) answers with one `Tick`. The protocol is the only mechanism: the engine never reads a clock or schedules anything, so a recorded command trace replays to the same events (§26).
- **`TextRasteriser`, `ImageSource`, `GpuBackend`: traits** (`cavea_render::host`, `cavea_render::backend`), for work the engine needs done synchronously in a frame. Image loading is asynchronous, so `ImageSource` only starts a load; the pixels come back as a command.

## 3. Engine protocol

The engine is driven only by commands and emits only events. Both are plain data, so the engine can later run in a worker or behind FFI unchanged (`DECISIONS.md` D-21).

**Commands**

| Command | Payload |
|---|---|
| `LoadChart` | CCF bytes (public or full artefact); replaces the loaded chart |
| `SetSnapshot` | `seq`, one public code per object (§17) |
| `ApplyDelta` | `from_seq`, `to_seq`, changes (`idx`, code) in increasing `idx` |
| `SetPrivateHolds` | The `idx` the buyer's hold token holds (increasing, at most 100), and its expiry |
| `Resize` | Width, height, device pixel ratio, maximum ratio (`maxDpr`) |
| `Pointer` | Kind (down, move, up, cancel), pointer id, type, position, time |
| `Wheel` | Delta in CSS pixels, position, modifier flags, time |
| `Key` | Key, modifiers |
| `SetCamera` / `FlyTo` | Target bounds, object or section; `FlyTo`: duration, or none for 250–600 ms by distance (§13) |
| `SetGestureHandling` | `greedy`, `cooperative`, `none` (§14) |
| `SetReducedMotion` | On or off (§13, `11-accessibility.md` §6) |
| `Select` / `Deselect` / `ClearSelection` | Labels or `idx` |
| `HoldResult` | Request id, success, or the failure code and the objects it names |
| `SetPricing` | Category keys with display prices |
| `SetFilter` | Categories, price range, accessible only |
| `SetTheme` | A built-in theme (`light`, `dark`, `high-contrast`) and token values to apply over it (§19) |
| `SetMode` | `picker`, `static`, `spotlight`, `manager` |
| `SetSessionMode` | `hold` (default: a selection emits `HoldRequest`) or `manual` (no hold or release requests; the host holds at checkout). Refused while anything is selected or a request is outstanding |
| `SetRules` | Maximum (1 to 100, default 100), minimum, validators, group size |
| `SetFloor` | Floor id |
| `Tick` | Frame time |
| `ContextLost` / `ContextRestored` | — |

**Events**

| Event | Payload |
|---|---|
| `Ready` | Chart metadata: name; floors (name, level, seats); sections (label, floor, seats); categories (key, label, colour, objects); seat and object counts |
| `NeedFrame` | — |
| `HoldRequest` / `ReleaseRequest` | Request id, labels |
| `SelectionChanged` | Selected objects with label, category, section, row, seat, flags; when the buyer's holds expire, if known |
| `SelectionInvalid` / `SelectionValid` | Rule and offending objects (§16) |
| `ObjectHover` / `ObjectFocus` | Object details, anchor position in screen space (the object's centre in CSS pixels); none when it leaves every object |
| `SectionEnter` | Section |
| `ObjectUnavailable` | A selected object was taken by someone else, or its hold failed with `not_free`; it is no longer selected |
| `HoldLost` | The buyer's hold on these selected objects is gone (the token expired or is unknown, or the objects were released elsewhere); they are no longer selected |
| `Resync` | A delta did not follow the state shown: the adapter fetches a new snapshot. Carries the engine's `seq`, if any; emitted once until a snapshot arrives |
| `CameraChanged` | Centre, zoom, level of detail |
| `Announce` | Text for assistive technology |
| `Metrics` | Timing marks and frame statistics |
| `Error` | Code (`Gpu`, `Unsupported`, `InvalidInput`, `BadChart`), message |

## 4. Load sequence

```
t0  init()  ─┬─ fetch WASM (streaming compile)
             ├─ fetch chart .ccf
             ├─ fetch state snapshot
             └─ fetch font atlas
    WASM ready ─ create GL context, compile 5 shader programs
    chart arrives ─ decode → SoA arrays → upload static buffers
    FIRST FRAME (layout visible, all seats neutral)        ← "chart visible"
    snapshot arrives ─ fill dynamic buffer
    INTERACTIVE FRAME                                       ← "chart-ready"
    open realtime connection
    load background images, build label buffers lazily
```

Rules:

- The four fetches start together, before any of them completes.
- The first frame does not wait for the snapshot or the font atlas.
- If the snapshot arrives before the chart, it is kept and applied on decode.
- Until state arrives every seat has status 0 and is drawn available: that is the neutral state of the first frame.
- A chart that does not decode is reported as `Error` with `BadChart`; the chart already loaded, if any, stays. The old chart's GPU resources are destroyed only once the new chart's are created.
- Shader programs are compiled before the chart arrives, where the browser allows parallel compilation.
- Nothing on this path allocates per seat or crosses the JavaScript boundary per seat.

## 5. Scene data (CPU)

Decoded from CCF into structure-of-arrays, indexed by `idx` (`cavea_render::scene::Scene`), in one linear pass with one allocation per array:

| Array | Type | Use |
|---|---|---|
| `pos` | `[f32; 2]` per seat, chart units | Hit testing, upload. Converted in place from the decoded quarter units |
| `cat` | u16 | Palette row, filters. The decoded `SCAT` array, not copied |
| `flags` | u8 | Accessibility filter, rules. The decoded `SFLG` array, not copied |
| `row` | u32 | Adjacency, labels |
| `status` | u8 | Public or manager status code |
| `visual` | u8 | Derived visual state (§9) |
| `fx` | u8 | Dim, highlight, pulse bits |

Plus per-row, per-segment, per-section, per-chunk and per-floor tables: the decoded chart itself, kept whole.

- **Active floor.** Only the active floor's `idx` range is drawn, and the camera fits that floor (§13). Until `SetFloor` exists it is the first floor; the other floors' seats are loaded and uploaded but not drawn.
- **Status.** `SetSnapshot` and `ApplyDelta` (§17) are the only ways status changes; each updates `status` and `visual` and grows the dynamic buffer's dirty range.
- **Memory at 100,000 seats:** 18 bytes per seat, 1.8 MB, for these arrays, plus the chart's tables (about 0.2 MB for the synthetic 100,000-seat stadium, measured locally) and 13 bytes per section for counters and tints. Loading also holds the 16 bytes per seat of buffer data being uploaded until the upload returns.

Derived at load (`cavea_render::overview::Sections`):

- **Section counters.** Per section, its *open* seats: those not drawn unavailable (visual state other than 1, §9), so available seats and the buyer's own selection (pending or held, and a seat given up whose release is not yet confirmed, §17) count as open. They are counted at load and on every snapshot (one pass over the rows' seat ranges), and updated in the same pass that changes a seat's visual state (delta, selection), by ±1 through the seat's row (`ROWS` section): no search. Only what the overview tint uses is built: per-category counters wait for filters and the "from" price. Cost measured locally (release build, AMD Ryzen 9 7900X, synthetic 100,000-seat stadium): a 9,502-change delta applies in 0.108 to 0.112 ms with them, against 0.107 ms without; a snapshot in 0.080 ms against 0.054 ms.
- **Dominant category** per section: the category most of its seats have, the lower index on a tie; none for a section without seats. Computed once at load in one pass over the seats; the compiled chart does not carry it.
- A neighbour table is not needed: seats within a row are stored in physical order, and `RSEG` marks the breaks.

## 6. GPU resources

| Resource | Size | Updated |
|---|---|---|
| Seat static buffer | 12 bytes × seats | Once |
| Seat dynamic buffer | 4 bytes × seats | Sub-ranges on state change |
| Mesh vertices (12 bytes: x, y `f32`, RGBA8) and `u32` indices | From `MESH` | Indices once; vertex sub-ranges when section tints change (§10) |
| Table and booth instance buffer | 24 bytes × objects | Once; dynamic part on state change |
| Palette texture | 33 × (categories + 1), RGBA8 | On theme or pricing change |
| Overlay instance buffers | 4 instances, static and dynamic layout | When a ring moves |
| MSDF glyph atlas | 256 × 256, RGB8 | Once |
| Dynamic text atlas | Up to 2048 × 2048, R8 | When new strings or zoom buckets are needed |
| Label glyph buffers | Per chunk, built lazily, LRU of 64 chunks | On first need |
| Icon atlas | 256 × 256, RGB8 | Once |
| Background image textures | As shipped, capped at 4096 px | After first frame |

Seat instance layout:

```
Static, 12 bytes                    Dynamic, 4 bytes
  pos    : f32 × 2                    cat    : u16
  half   : u16  (half size, ¼ cu)     visual : u8
  angle  : u8   (360° / 256)          fx     : u8
  shape  : u8
```

- `half` and `shape` come from the seat's row (`ROWS` seat size and shape: 0 circle, 1 rounded, 2 chair).
- `angle` is 0 for circles. For oriented shapes (rounded, chair) it is the direction of the row at the seat, from the seat before it to the seat after it in its segment (one-sided at a segment's ends); a seat alone in its segment takes its row's direction from first to last seat. The seat's local +y is therefore behind it, away from the front rows.

## 7. Pipelines

Five shader programs. Draw order, back to front:

| # | Pass | Program | Primitive | Drawn when |
|---|---|---|---|---|
| 1 | Clear | — | — | Always |
| 2 | Background images | `image` | Quad | Loaded and enabled |
| 3 | Decoration, lower layer | `mesh` | Indexed triangles | Always |
| 4 | Section fills | `mesh` | Indexed triangles | Overview and cross-fade (§10), at the fills' opacity |
| 5 | Area fills | `mesh` | Indexed triangles | Always |
| 6 | Tables and booths | `shape` | Instanced quad | Seats and detail levels |
| 7 | Seats | `shape` | Instanced quad, one or more ranges | Cross-fade (at the seats' opacity), seats and detail levels |
| 8 | Seat icons (accessible, selected tick, unavailable cross) | `glyph` | Instanced quad | Detail level |
| 9 | Seat labels | `glyph` | Instanced quad | Detail level |
| 10 | Row labels, section labels, text objects, area labels | `glyph` or `text` | Instanced quad | By size |
| 11 | Decoration, upper layer and icons | `mesh`, `glyph` | — | Always |
| 12 | Overlays: hover ring, focus ring, selection box | `shape` | Instanced quad | When present |

Mesh passes draw only the active floor's ranges: its `below` draw range (pass 3), the outline ranges of its sections (pass 4: the active floor's sections are consecutive in `SECT`, so their outlines are one index range and **all section fills are one draw call**) and its `above` draw range (pass 11). So a frame draws at most three mesh calls. Mesh draws are not culled: the whole mesh is small (39,552 bytes for the synthetic 100,000-seat stadium's 24 section outlines, measured locally), the GPU clips off-screen triangles, and splitting the fill range by visibility would add draw calls. Tinting by vertex colour rather than one draw per section with a tint uniform keeps it one call and adds no vertex attribute; the cost is a vertex sub-range upload when a tint changes (§10).

The hover and focus rings are drawn in screen space (CSS pixels), so they are the same width at any zoom: for each, a halo in the background colour from the seat's edge out 2 px, then the ring 2 px wide outside it (`RING_WIDTH_PX`), together the two-tone ring of `11-accessibility.md` §3. Each is a `shape` instance of the seat's shape and angle (a chair's ring is a rounded square) whose fill is transparent and whose stroke is the ring; at most four instances, uploaded only when they change. The focus ring is drawn over the hover ring.

Programs:

| Program | Vertex inputs | Fragment work |
|---|---|---|
| `shape` | Corner from vertex id; per-instance position, size, angle, shape, category, visual, fx | Signed-distance shape, fill, stroke, anti-aliasing |
| `mesh` | Position, colour | Vertex colour × pass opacity, premultiplied; section outline vertices hold their section's tint (§10), written by the engine over the white of `03-chart-format.md` §4.2.3 |
| `glyph` | Corner; per-instance position, size, atlas rectangle, colour index | MSDF median, anti-aliased edge |
| `text` | Same as `glyph` | Alpha from R8 bitmap atlas |
| `image` | Corner; rectangle | Texture × opacity |

## 8. Seat shader

- Each seat is a quad built from `gl_VertexID` (triangle strip of 4), scaled by `half`, rotated by `angle`, translated by `pos`, transformed by the camera matrix.
- The vertex shader fetches fill and stroke colours from the palette with `texelFetch(palette, ivec2(visual × 3 + k, cat), 0)`, `k` 0 fill, 1 stroke (2 is the ink for text on the seat), and passes them flat.
- The fragment shader evaluates a signed distance for the shape:

| Shape id | Distance function |
|---|---|
| 0 circle | `length(uv) − 1` |
| 1 rounded square | Rounded box, radius 0.3 |
| 2 chair | Rounded box with a thicker edge on the back side (local +y, §6) |
| 3 diamond | `abs(uv.x) + abs(uv.y) − 1` |

- Anti-aliasing uses the on-screen pixel size passed as a uniform: `alpha = clamp(0.5 − d × half_px, 0, 1)`.
- Stroke is drawn where `−stroke_px < d × half_px < 0`. A seat smaller than 5 px on screen, by its own size, gets no stroke (`tuning::STROKE_MIN_SEAT_PX`, the seats level's threshold, §10).
- `fx` bit 0 (dim) multiplies alpha by the theme's dim factor. Bit 2 (pulse) blends the colour towards the seat's stroke colour by up to 70%, twice a second from a time uniform (under the three-flashes-a-second limit of `11-accessibility.md` §6), so a seat just taken flashes solid then shows hollow in every theme.
- A per-pass opacity uniform multiplies alpha: the seats' cross-fade opacity (§10), 1 otherwise. The `mesh` program reads the same uniform block.
- Blending is premultiplied alpha. No depth buffer, no stencil.

## 9. Visual states and palette

The engine maps status, selection and mode to one visual state per object:

| Code | Visual state | Picker mode | Manager mode |
|---|---|---|---|
| 0 | Available | Free and passes filters | Free |
| 1 | Unavailable | Anything not available | — |
| 2 | Selected, pending | Tapped, hold in flight | Selected by the operator |
| 3 | Selected, held | Hold confirmed | — |
| 4 | Blocked | — | Blocked |
| 5 | Booked | — | Booked |
| 6 | Held by another | — | Held |
| 7 | Custom | — | Custom status (colour from the status table) |
| 8–10 | Overlays | Ring halo (`bg`), hover ring, focus ring: stroke only | The same |

Picker mode: the buyer's own selection wins over the status (pending 2, held 3); otherwise status 0 is available and any other status unavailable. Filters do not change the state; they set the dim bit of `fx`. Manager mode keeps codes 4–7; their colours are placeholders until that mode is built.

The palette texture has one row per category and one extra, last row for objects without a category. Each row has three texels per visual state: fill, stroke, ink. A state whose colour does not depend on the category is written identically into every row, so the shader always indexes by `cat`. Changing a theme or a category colour rewrites the texture and touches no instance data.

| State | Fill | Stroke | Ink |
|---|---|---|---|
| Available, custom | Category colour | `seat-available-stroke` | Black or white, whichever contrasts more with the fill as drawn over `bg` |
| Unavailable, booked | `seat-unavailable-fill` | `seat-unavailable-stroke` | `fg-muted` |
| Selected (pending, held) | `seat-selected-fill` | `seat-selected-fill` | `seat-selected-ink` |
| Blocked | `fg-muted` | `fg` | `bg` |
| Held by another | Category colour | `fg` | As available |

States must differ by more than colour (`11-accessibility.md` §6): unavailable seats are drawn hollow or with a cross at detail level, selected seats carry a tick.

## 10. Level of detail

Let `s` be the seat size in CSS pixels at the current zoom.

`s` is the chart's default seat size (`META`) times the zoom. Every threshold is a named constant in `cavea_render::tuning` (`LOD_*`).

| Level | Condition (upwards) | Drawn |
|---|---|---|
| **Overview** | `s < 2.5` and the active floor has section fills | Section fills tinted by availability; section labels and "from" price with text and pricing; no seats |
| **Cross-fade** | `2.5 ≤ s < 5` | Section fills fade out while seats fade in |
| **Seats** | `5 ≤ s < 14` | Seats, each with its stroke from 5 px on screen (§8); row labels from `s ≥ 8` with text |
| **Detail** | `s ≥ 14` | As seats, plus seat labels and icons when the text and icon passes exist |

- **Stroke.** The stroke is not tied to the level: each seat gets it from 5 px on screen by its own size (rows can have larger seats than the default), which is where the seats level begins. Unavailable seats are hollow (§19), so between 5 and 14 px they need the stroke to differ from available seats by more than colour (`11-accessibility.md` §6). Selection follows the same thresholds: a mouse click selects from 5 px (`tuning::SELECT_MIN_SEAT_MOUSE_PX` is the seats threshold), a touch from 18 px (§15).
- **Hysteresis.** It applies to the level: a level is entered upwards when `s` reaches its threshold and left downwards only when `s` falls below 90% of it (`LOD_HYSTERESIS`), so between 2.25 and 2.5, 4.5 and 5, and 12.6 and 14 px the level is the one the camera came from. After `LoadChart` the level has no history and follows the upward thresholds.
- **Cross-fade.** A function of `s` alone, so it needs no animation and a still camera draws nothing: the seats' opacity rises linearly from 0 at 2.5 px to 1 at 4.5 px (the lower edge of the 5 px threshold's hysteresis band) and the fills' opacity is 1 minus it. It is 0 at overview and 1 at seats and detail whichever way the level was reached, so no level change makes anything jump. Seats are not drawn while their opacity is 0, nor fills while theirs is.
- **No sections.** A chart whose active floor has no section fills skips overview and cross-fade: its level is at least seats and its seats are drawn opaque at any size (below 5 px, without stroke).
- **Section tint** (`overview::Sections::tint`). With `C` the dominant category's colour in the current theme (a `category-<key>` override applies), `G` the sold-out grey, `k` the theme's `section-tint` strength (0 to 1) and `o / n` the section's open seats over its seats, each RGB channel is `G + (C − G) × k × o / n`, rounded, in integer arithmetic; the fill is opaque. So a fully available section shows `k` of its colour, a half-sold one `k / 2`, and a sold-out one, or one without seats, is exactly `G`.
- **Sold-out grey** `G` is the theme's `seat-unavailable-fill` (light `#E7E5E4`, dark `#292524`, high contrast `#1F1F1F`): a sold-out section is filled close to the background like an unavailable seat, so it differs from the sections around it by lightness, not hue alone. The "sold out" label comes with text.
- **Updates.** A counter change marks its section's tint stale; at the next frame each stale tint is computed once and written into that section's outline vertex colours, and the vertex range written is uploaded once. A delta at overview therefore redraws one frame however many changes it carries. A theme change marks every tint stale.
- **Taps in overview.** Section drill-down (a tap flies the camera to the tapped section) is REN-053. Until then a tap at overview is handled as at any level: it hits the nearest seat within the tolerance (§15), which is too small to select at overview, so zoom-to-select flies the camera to make seats 32 px about it; a tap off every seat does nothing. REN-053 replaces this at overview only.
- **Pulses** (§17) start only while seats are drawn (opacity above 0); a pulse running when the camera reaches overview ends at its time.
- Measured on the demo (Chromium 157, SwiftShader software rendering, 1000 × 800 CSS px, the synthetic 100,000-seat stadium with a pitch below and a band above): 3 draw calls at overview (lower decoration, section fills, upper decoration), 9 to 14 in the cross-fade, 5 to 9 at seats and detail (two decoration calls plus the visible seat ranges and any overlay).

## 11. Culling and draw calls

- Chunks hold at most 4,096 seats and have a bounding box. A chunk is drawn if its box intersects the view.
- Visible chunks that are adjacent in `idx` order are merged into one instance range.
- At overview and when the whole chart is visible, all seats are one range and one draw call.
- Target: at most 64 seat draw calls in any frame. While more ranges are visible, the two separated by the fewest seats are merged: the seats between them are drawn too, off screen, which costs some overdraw instead of draw calls.
- Chunk boxes in `CHNK` bound seat centres, so they are grown before the test by the reach of the largest seat quad at any angle: √2 × (largest half size + one device pixel for the anti-aliased edge, §8). Only the active floor's seats are considered; a chunk is clipped to it.
- The visible ranges are computed when the camera, the viewport or the chart changes, and kept otherwise: a frame drawn for a state or theme change reuses them.
- WebGL2 has no base-instance draw, so the `glow` backend rebinds attribute offsets per range. The `wgpu` backend passes instance ranges directly.

## 12. Text

| Text | Source | Notes |
|---|---|---|
| Seat labels | MSDF atlas | Generated from row rules; upright; ink colour chosen per palette entry for contrast |
| Row labels | MSDF atlas, or dynamic atlas if non-Latin | At row ends per chart setting; hidden when they collide |
| Section labels | Dynamic atlas | Anchor and maximum size precomputed by the compiler |
| Text objects | Dynamic atlas | As authored |
| Prices in overview | MSDF atlas | From `SetPricing` |

- **MSDF atlas:** digits, A–Z, a–z and common punctuation of one open-licensed typeface, baked at build time.
- **Dynamic atlas:** the host's `TextRasteriser` draws whole strings at a requested pixel size (browser canvas `fillText`, CoreText, Android canvas). The platform does shaping and font fallback. Sizes are bucketed by powers of √2 and re-rasterised when the zoom crosses a bucket.
- **Collision:** row labels and text objects are placed greedily by priority on a coarse screen grid. Seat labels never collide by construction.
- **Label buffers:** built per chunk on first need and kept in an LRU of 64 chunks. Building 4,096 seats' labels takes well under a millisecond.

## 13. Camera

- State: centre in world space, zoom in CSS pixels per cu. Rotation is fixed at zero in v1; the matrix supports it.
- `fit`: the zoom at which the active floor fills the viewport with 24 px padding.
- Limits: minimum `0.8 × fit`; maximum where a default seat is 64 px.
- The centre is clamped so that, in each axis, at least a third of the viewport shows the chart (or the whole chart, where it is smaller than that on screen).
- `FlyTo` interpolates the centre linearly and the zoom in log space, with a cubic ease-out curve. Its duration is 250 ms plus 100 ms per doubling of zoom and per viewport diagonal of travel (at the closer zoom), at most 600 ms, unless the command gives one.
- One zoom step (double tap, two-finger tap) is a factor of 2, animated like `FlyTo`.
- With reduced motion requested, all camera changes are instant and drags have no inertia.
- The camera maths has no renderer dependencies (`cavea_render::camera`) so it can move to `cavea-core`.

## 14. Input and gestures

| Input | Action |
|---|---|
| One-finger or mouse drag | Pan, with inertia on release |
| Two-finger pinch | Zoom about the midpoint; simultaneous pan |
| Mouse wheel | Zoom about the cursor |
| Trackpad two-finger scroll | Pan |
| Trackpad pinch (wheel with ctrl) | Zoom about the cursor |
| Double tap or double click | Zoom in one step about the point |
| Two-finger tap | Zoom out one step |
| Tap or click | Select, or zoom if the target is too small (§15) |
| Long press (touch) | Show the tooltip |
| Hover (mouse) | Hover ring and tooltip |
| Keyboard | See `11-accessibility.md` §3 |

- Pointer events are the single input source on the web, with pointer capture on press so a drag that leaves the canvas continues. Each active pointer is tracked by id (up to 10). A lost capture, a hidden page or a second mouse press cancels the pointers left down.
- A press becomes a drag after 8 px of movement (touch, pen) or 4 px (mouse). The drag grabs the chart where the press began.
- Inertia: release velocity is measured over the drag's last 100 ms; it decays exponentially with a 325 ms time constant and stops below 10 px/s.
- A pinch zooms about the previous midpoint by the change in finger distance and pans by the midpoint's movement, so the chart point under the midpoint follows it.
- Wheel: with ctrl (a trackpad pinch, or ctrl+mouse wheel) it zooms about the cursor. Without ctrl, a stream of wheel events (gaps under 300 ms) that starts vertical-only with a delta of 50 px or more, or a multiple of the macOS notch (4.000244 px), is a mouse wheel and zooms (300 px per doubling); anything else is a trackpad and pans. Browsers do not say which device it is, so this is a heuristic. The host converts line deltas to 50 px.
- Tap: a press released without becoming a drag; a touch or pen held 500 ms or more is a long press instead (recognised on the first `Tick` past 500 ms, or at release if no `Tick` came). A mouse press is a click however long it is held.
- Double tap: a second tap that goes down within 300 ms of the first one's release and within 24 px of it. The first tap is reported as a tap (selection is not delayed); the second zooms instead.
- Two-finger tap: two pointers, neither moved past the threshold, the last lifted within 300 ms of the first press; zooms out one step about their midpoint.
- Recognised taps (with the pointer type), long presses and hover positions (mouse or pen moving with nothing pressed; cleared when it leaves) go to the engine's intent handler, where hit testing and selection (§15, §16) take them.
- A cancelled pointer ends the gesture: no tap, no inertia, and any other pointers are ignored until all have lifted.
- `gestureHandling`: `greedy` (default; the map takes all gestures), `cooperative` (page scrolls on one finger or plain wheel; two fingers, ctrl+wheel and mouse drags move the map), `none` (no gesture moves the camera). Taps, long presses and hover are recognised in every mode; a double tap that cannot zoom is two taps.
- `touch-action` is set on the canvas to match the mode: `none`, `pan-x pan-y`, `auto`. The wheel listener is not passive and prevents the default only for wheels the map takes.
- Every gesture constant is in `cavea_render::tuning` (§26).

## 15. Hit testing

1. Convert the screen point to world space.
2. Find the active floor's sections whose bounding box, grown by the largest tolerance of any seat, contains the point (a short linear scan; sections are few).
3. Within those, find rows whose bounding box (of seat centres), grown by the row's tolerance, contains the point. The active floor's rows outside any section are tested here too.
4. Within those rows, find the seat with the nearest centre.
5. Accept it if the distance is within the tolerance. Equal distances go to the lower `idx`. The result is the brute-force answer: the nearest seat centre among the seats within their tolerance (`cavea_render::hit`).

- Tolerance: the larger of the seat's half size and 12 CSS px (touch and pen) or 4 CSS px (mouse, and hover).
- Tables, booths and areas are tested against their shapes after seats.
- **Zoom-to-select:** on touch or pen, if the hit seat's size on screen `s < 18` the tap does not select. It flies the camera to make seats 32 px, centred on the tap. With a mouse, selection works from `s ≥ 5`; a click on a smaller seat zooms the same way. A tap that hits no seat does nothing. The thresholds are in `cavea_render::tuning`.
- Hover hit-tests each mouse or pen position and again when the camera moves; `ObjectHover` fires and the ring is redrawn only when the hovered object changes. A long press reports its object with `ObjectHover` (for the tooltip); the next touch tap clears it.
- Target: under 20 µs per hit test at 100,000 seats. Measured locally (release build, AMD Ryzen 9 7900X, the synthetic 100,000-seat stadium, taps near seats at 18 to 64 px seats): 0.55 µs.

## 16. Selection engine

```
tap → hit test → rules check ──fail──▶ SelectionInvalid (no state change)
                     │ pass
                     ▼
        visual = selected-pending, SelectionChanged, HoldRequest
                     │
        ┌────────────┴────────────┐
   HoldResult ok             HoldResult failed
   visual = selected-held    revert, ObjectUnavailable or SelectionInvalid
```

- Rules run locally first (`02-domain-model.md` §12) using the same code the server uses (`cavea_core::rules`). A seat that is not available (public status other than 0, and not still held by this buyer) is refused with `SelectionInvalid` rule `not_free`; one over the maximum with rule `max`. Neither changes anything.
- The visual state changes in the frame of the `Tick` that takes the tap: one dynamic-buffer sub-range upload.
- Per seat, the selection is: not selected, selected and pending (hold not yet requested, or in flight), or selected and held. At most `MAX_SELECTED` (100) seats are selected.
- Hold requests for one token are sent in order, one in flight at a time; taps made meanwhile are coalesced into the next request. Precisely:
  - Taps of one frame, and all taps made while a hold is in flight, go out together in the next `HoldRequest`.
  - A seat tapped on and off again before its request went out is in no request.
  - A seat deselected while its hold is in flight stays in that request; if the hold succeeds the seat is released in the next `ReleaseRequest`, otherwise nothing is sent. Tapped on again before the result, it is selected again and the result applies to it.
  - A held seat deselected while a hold is in flight is released after the result; tapped on again before that, it is held again with no request.
  - Releases go out before holds, and only while no hold is in flight, so a release never overtakes the hold of the same seat. Releases have no result; the host sends requests in the order emitted.
- `HoldResult` ok: the request's seats become held. Failed: holds are all or nothing (`02-domain-model.md` §4.3), so no seat of the request is held. Each seat the failure names reverts: `ObjectUnavailable` for `not_free` (or `object_not_free`), otherwise `SelectionInvalid` with the code as the rule. Seats the failure does not name stay pending and go out in the next request; if it names none of the request's seats, all revert with the first code. A result for an unknown request id, or a second result, is an `InvalidInput` error and changes nothing.
- **Readiness:** `SelectionValid` and `SelectionInvalid` with rule `min` (objects: the selection) report, when it changes, whether the selection meets the minimum for checkout. Every other `SelectionInvalid` reports objects that were refused; the selection is as it was.
- `Select`, `Deselect` and `ClearSelection` behave as taps and take labels or `idx`; `Select` is all or nothing (any unavailable seat, or too many, refuses the whole command). An unknown label or `idx` is an `InvalidInput` error for the whole command. Labels resolve through an index of every seat's full label, built on the first lookup by label (8 bytes per seat; 9 ms for 100,000 seats, measured locally).
- **Group selection:** with a group size *n*, one tap selects the *n* seats in the tapped seat's segment that contain it and create the fewest orphans, preferring the window centred on the tap.
- `session: "manual"` (`SetSessionMode`) skips hold requests; a tap selects at once (visual state selected-held) and deselecting sends no release.
- **Token gone.** A failure with code `not_found` means the hold token expired or never existed, and every hold with it (`08-server.md` §6.4; the engine's labels all come from the chart, so no object can be unknown). The whole selection leaves with one `HoldLost` naming it and one `SelectionChanged`; no release is sent, the private set is forgotten, and the next tap sends a new `HoldRequest` (the adapter creates a token or reports the session over).
- **Hostile results.** A result for an unknown request id, a release's id, or a second result is an `InvalidInput` error and changes nothing. Failures naming objects outside the request are ignored; if none is in it, the whole request reverts. Every scan is bounded by the request (at most 100 seats) and the input's length.
- If a status change (a snapshot or a delta) makes a selected object unavailable and that is not the buyer's own hold, it is removed from the selection and `ObjectUnavailable` fires. Own holds (§17): a held seat, a seat whose hold is in flight (its `HoldResult` decides), and any seat in the private set or released but not yet confirmed. So a seat whose request has not gone out, or any seat in a manual session outside the private set, is knocked out.
- Deselecting sends a release.
- `LoadChart` forgets the selection, hover and focus without release requests (the holds stay on the token, which the host owns) and emits `SelectionChanged` if anything was selected. It also forgets `seq`, the state and the private set: the adapter sends them again for the new chart (CLOSE 2 is followed by a resubscribe, `08-server.md` §8.6).

## 17. Applying state

State arrives as commands with decoded data. `Command::from_frame` turns a SNAPSHOT, DELTA or PRIVATE frame of `08-server.md` §8 into `SetSnapshot`, `ApplyDelta` or `SetPrivateHolds` (one allocation, nothing per seat across the JavaScript boundary); on the web `View.applyFrame(bytes)` does that for the `caveaServer` adapter, and `View.setSnapshot(seq, codes)` takes one `Uint8Array` of codes from a custom adapter that has no server frames. A frame that does not decode is refused whole, as is one naming any `idx` at or past the chart's object count, in its area entries too (`08-server.md` §8.6). Before a chart is loaded no count is known, so a DELTA or PRIVATE frame with area entries is refused then (servers send none yet); the engine then checks the decoded state against its chart, and anything that does not fit is an `InvalidInput` error that changes nothing: a snapshot whose length is not the object count, a code above 1, a delta or private set whose `idx` are not increasing or reach past the objects, a private set of more than 100. Codes of objects past the seats (tables, booths, areas) are accepted and not drawn yet.

- **Snapshot:** replaces all state. One pass copies the codes and computes `visual` for every seat, the selection's seats are then drawn as theirs, and the whole dynamic buffer is uploaded once at the next frame. It records `seq`. Selected seats it shows unavailable that are not the buyer's own (§16) leave with `ObjectUnavailable`. A snapshot (and a private set) that arrives before any chart is kept and applied when the chart loads (§4).
- **Delta:** applied only when its `from_seq` is the engine's `seq`, which becomes `to_seq`; it may carry no changes. A gap, a repeat, or a delta before any snapshot changes nothing and emits `Resync` (once until a snapshot arrives); the adapter fetches a new snapshot and the engine never guesses. Changed seats update `status`, `visual` and their section's counter (§5) and grow the dirty range; selected seats it makes unavailable leave as for a snapshot.
- **One dirty range.** All changes between two frames merge into one range, uploaded once per frame: two far-apart changes upload everything between them. Measured locally (release build, AMD Ryzen 9 7900X, synthetic 100,000-seat stadium, the test backend's upload): a 9,502-change delta spread over the chart (10,000 random changes, coalesced) decodes and applies in 0.11 ms, and with encoding the 400 KB it uploads, 0.19 ms, against the 2 ms budget for 10,000 changes; a 100,000-seat snapshot (12.5 KB frame) applies in 0.04 ms, 0.12 ms with its upload. A spread-out delta covers most of the buffer under any small set of ranges, so a set of ranges would save upload bytes only for sparse deltas; that is left until a device measurement shows the bytes matter.
- **Own holds.** The buyer's hold makes the seat's public status unavailable, so status alone cannot tell the buyer's seats from others'. `SetPrivateHolds` carries the full set of seats the buyer's token holds, replacing the previous one:
  - A seat that enters the set and is selected becomes held (the set confirms a hold whose result has not arrived; the result then changes nothing, or reverts the seat if it failed). One that is not selected is selected as held: that restores the selection after a reload, with one `SelectionChanged`. The restore stops at 100 selected seats; it ignores the maximum rule, since the seats are already held.
  - A held seat that leaves the set without the buyer deselecting it (the token's hold expired, or another tab released it) leaves the selection with `HoldLost`. A pending seat that leaves it is decided by its `HoldResult`.
  - Seats the buyer gave up (deselected while their hold is in flight, waiting to be released, or released but not yet absent from a set) are ignored by the set: a set still listing them is older than the release. A set without the seat, a status change for it, or a snapshot confirms the release; a status change or snapshot also drops the seat from the older set, so selecting it again requests a new hold. Until then the seat is drawn available, and tapping it again takes it back (with no request if its release has not gone out).
  - Orderings. For one tapped seat, the hold result, the private set confirming it and the delta marking it unavailable may arrive in any order: the seat is selected-pending until the first of the result and the set, then selected-held, and never drawn unavailable. A delta marking a pending or held seat unavailable never knocks it out. After a reload, the snapshot and the private set may arrive in either order, before or after the chart.
  - The one ordering the engine cannot tell apart: a seat deselected, released and selected again whose new hold result arrives before the set that confirms the release. That set looks like a lost hold, so the seat leaves with `HoldLost` and comes back held with the next set. It is transient, and nothing is left stuck.
  - The expiry of a non-empty set is reported in `SelectionChanged` (`expiresAt` on the web) whenever it changes, for the SDK's hold timer.
- **Pulse:** seats that a delta turns from drawn available to drawn unavailable (not the buyer's own, not on a snapshot) set the pulse bit of `fx` for 400 ms (`tuning::PULSE_MS`), unless reduced motion is requested or seats are not drawn (overview, §10); turning reduced motion on ends running pulses. The seats of all deltas between two frames form one batch whose end is set by the next `Tick`, so a burst of 10,000 changes is one batch, and at most one batch per frame exists. While any batch runs the engine asks for frames (the shader animates the bit with the time uniform); when the last ends, its seats' bits are cleared in one upload of the batch's range and the frame loop goes idle.

## 18. Frame scheduling

- The engine draws only when a dirty flag is set (camera, state, selection, theme, size, text) or an animation is running.
- It asks the host for a frame with `NeedFrame`; the host calls `Tick` from `requestAnimationFrame` or the platform's display link.
- Input received between frames is applied at the start of the next `Tick`, so pointer-to-photon latency is one frame. Input, `SetCamera` and `FlyTo` are queued in arrival order (at most 256; a fuller queue is applied at once) and each requests a frame.
- Resizes are observed, coalesced, and applied at the next frame; the web host resizes the canvas in the same frame, so it is never cleared between frames.
- Inertia and `FlyTo` are entries in one animation list; a gesture start or a new camera move replaces them.
- `NeedFrame` is emitted at most once per `Tick`, and only for queued work, a dirty flag, a running animation or a pending long press. With none of these, no frame is requested and none is drawn.
- The web host stops `requestAnimationFrame` while the page is hidden and resumes it if the engine still wants a frame.

## 19. Theming

Tokens (the full list with CSS names is in `05-web-sdk.md` §8):

- Background, text, muted text.
- Per visual state: fill, stroke, ink, with or without category colour.
- Selection ring (`seat-selected-fill`, which is also the selected seats' stroke), hover ring, focus ring.
- Section tint strength, dim factor.
- Font family for dynamic text.

Built-in themes: `light`, `dark`, `high-contrast`, and a `colorblind` modifier that switches to a tested palette and stronger shape cues. Category colours come from the chart and can be overridden by the host.

- `SetTheme` names a built-in theme and token values applied over it, in order. Each `SetTheme` replaces the whole theme, which then outlives `LoadChart`. An unknown token or invalid value is refused with `InvalidInput` and changes nothing.
- Token names are those of `05-web-sdk.md` §8, with or without the `--cavea-` prefix; `category-<key>` overrides one category. Colours are `#rgb`, `#rrggbb` or `#rrggbbaa`; `section-tint` and `dim` are decimals from 0 to 1. `font`, `radius` and `shadow` belong to the host.
- The theme sets the palette (§9), the clear colour (`bg`) and the dim factor.
- Built-in defaults: unavailable seats are filled close to the background (1.2:1 light, 1.15:1 dark, 1.27:1 high contrast) with a strong stroke, so they read as hollow and differ from solid available seats by more than hue (`11-accessibility.md` §6); below 5 px on screen there is no stroke and the difference is lightness.
- WCAG contrast against the background, light / dark / high contrast, checked against the 3:1 non-text and 4.5:1 text requirements of `11-accessibility.md` §6: unavailable stroke 4.6 / 6.9 / 21; selected fill 16.7 / 16.7 / 19.6; focus ring 4.9 / 6.9 / 19.6; muted text 7.3 / 6.9 / 21. The synthetic charts' category colours keep at least 4.5 / 3.19 / 3.8. Ink on a fill is black or white by exact sRGB relative luminance: 4.7:1 or more on those category colours, 16.7:1 or more on the selected fill, 6:1 or more for unavailable seats.

## 20. Backends

### 20.1 Trait

```rust
pub trait GpuBackend {
    fn caps(&self) -> Caps;
    fn create_buffer(&mut self, desc: BufferDesc, data: &[u8]) -> Result<BufferId, BackendError>;
    fn update_buffer(&mut self, id: BufferId, offset: u32, data: &[u8]) -> Result<(), BackendError>;
    fn create_texture(&mut self, desc: TextureDesc, data: Option<&[u8]>) -> Result<TextureId, BackendError>;
    fn update_texture(&mut self, id: TextureId, region: Rect, data: &[u8]) -> Result<(), BackendError>;
    fn begin_frame(&mut self, target: Target, clear: [f32; 4]);
    fn draw(&mut self, call: &DrawCall) -> Result<(), BackendError>;   // fixed pipeline enum, uniforms, buffers, ranges
    fn end_frame(&mut self);
    fn destroy(&mut self, id: ResourceId);
}
```

Pipelines are a fixed enum. Backends compile them at start; nothing creates pipelines at run time. Operations that can fail (a lost context, an unknown id, data that does not fit) return an error instead of panicking; the engine reports it as an `Error` event.

### 20.2 `glow` (web, WebGL2)

- Context attributes: `antialias: false` (edges are anti-aliased in the shader), `alpha: false`, `depth: false`, `stencil: false`, `powerPreference: "high-performance"`, `desynchronized: true` where supported.
- GLSL ES 3.00 shaders generated at build time.
- Handles `webglcontextlost` and `webglcontextrestored`: all buffers are re-uploaded from the CPU arrays, which the engine keeps.

### 20.3 `wgpu` (native)

- Metal on iOS, Vulkan on Android with GLES as fallback.
- Surface from `CAMetalLayer` or `ANativeWindow`.
- Present mode: FIFO (vsync).

### 20.4 Canvas 2D fallback (web)

- Written in TypeScript, driven by the engine's draw list for the visible chunks only.
- Seats are batched into one path per colour.
- No cross-fades or pulses.

### 20.5 Capability ladder

| Step | Condition | Result |
|---|---|---|
| 1 | WebGL2 context created and a test frame renders | Full renderer |
| 2 | WebGL2 unavailable or the context fails twice | Canvas 2D fallback |
| 3 | Canvas unavailable, or WASM blocked | Accessible list view (`11-accessibility.md` §4) |

The chosen step is reported in `Ready` and in metrics.

## 21. Resolution and power

- The backing store is `min(devicePixelRatio, maxDpr)` × CSS size. `maxDpr` defaults to 2 on touch devices (`(pointer: coarse)` on the web) and 3 otherwise; the host sends it with every `Resize`.
- The backing store is capped at 4,096 px on its long side.
- A change of device pixel ratio (a window moved between screens, browser zoom) is a resize; the web host watches it with a `(resolution: …dppx)` media query.
- **Dynamic resolution:** if the 95th-percentile frame time exceeds the budget for 30 consecutive frames during a gesture, the engine renders at a lower ratio until the gesture ends, then redraws at full resolution.
- When the page is hidden, animations stop and the realtime adapter may pause (host's choice).

## 22. Memory

At 100,000 seats:

| Item | Size |
|---|---|
| CPU scene arrays | ~2.5 MB |
| GPU seat buffers | 1.6 MB |
| Label buffers (64 chunks) | ≤ 6 MB |
| Text and glyph atlases | ≤ 5 MB |
| Meshes | ≤ 2 MB (the synthetic 100,000-seat stadium: 1,672 vertices and 4,872 indices, 39,552 bytes on the GPU and the same decoded on the CPU, measured locally) |
| WASM code and heap overhead | ≤ 4 MB |

This leaves wide margin under the 40 MB budget. Background images are the only large variable; they are capped and optional.

## 23. Worker mode (P2)

- The engine and the `glow` backend run in a web worker on an `OffscreenCanvas`.
- The main thread forwards input and receives events.
- Dynamic text is rasterised in the worker with an `OffscreenCanvas` 2D context.
- The accessibility layer stays on the main thread and is fed by events.
- Enabled by option; falls back to the main thread where unsupported.

## 24. Instrumentation

Timing marks (also emitted as `performance.mark` on the web):

`init_start`, `wasm_ready`, `chart_fetched`, `chart_decoded`, `gpu_ready`, `first_frame`, `state_ready`, `interactive`.

Frame statistics over a rolling 120-frame window: median, 95th percentile and maximum frame time, draw calls, visible seats, level of detail, backend, resolution ratio.

Delivered through the `Metrics` event at most once a second.

## 25. Designer extensions

The designer uses the same engine with three additions:

- **Live scene.** A mutable `SceneSource` in which each node is a chunk. Edits re-expand only the changed node.
- **Chunk transforms.** While a selection is dragged, its chunks are drawn with a per-chunk matrix; positions are baked on release.
- **Overlay batcher.** An immediate-mode layer for selection outlines, handles, guides, rulers and measurement labels, drawn in screen space.

## 26. Testing hooks

- A headless mode renders a given chart, state and camera to a pixel buffer for golden-image tests.
- A scripted input trace (recorded gestures with timestamps) drives benchmarks deterministically.
- Every level-of-detail threshold and gesture constant is a named constant in one module.
