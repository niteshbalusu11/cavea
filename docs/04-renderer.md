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
| `SetSnapshot` | `seq`, state bytes |
| `ApplyDelta` | `from_seq`, `to_seq`, changes |
| `SetPrivateHolds` | List of `idx`, expiry |
| `Resize` | Width, height, device pixel ratio, maximum ratio (`maxDpr`) |
| `Pointer` | Kind (down, move, up, cancel), pointer id, type, position, time |
| `Wheel` | Delta in CSS pixels, position, modifier flags, time |
| `Key` | Key, modifiers |
| `SetCamera` / `FlyTo` | Target bounds, object or section; `FlyTo`: duration, or none for 250–600 ms by distance (§13) |
| `SetGestureHandling` | `greedy`, `cooperative`, `none` (§14) |
| `SetReducedMotion` | On or off (§13, `11-accessibility.md` §6) |
| `Select` / `Deselect` / `ClearSelection` | Labels or `idx` |
| `HoldResult` | Request id, success or per-object failures |
| `SetPricing` | Category keys with display prices |
| `SetFilter` | Categories, price range, accessible only |
| `SetTheme` | Token values |
| `SetMode` | `picker`, `static`, `spotlight`, `manager` |
| `SetRules` | Maximum, minimum, validators, group size |
| `SetFloor` | Floor id |
| `Tick` | Frame time |
| `ContextLost` / `ContextRestored` | — |

**Events**

| Event | Payload |
|---|---|
| `Ready` | Chart metadata: name; floors (name, level, seats); sections (label, floor, seats); categories (key, label, colour, objects); seat and object counts |
| `NeedFrame` | — |
| `HoldRequest` / `ReleaseRequest` | Request id, labels |
| `SelectionChanged` | Selected objects with label, category, section, row, seat, flags |
| `SelectionInvalid` / `SelectionValid` | Rule and offending objects |
| `ObjectHover` / `ObjectFocus` | Object details, anchor position in screen space |
| `SectionEnter` | Section |
| `ObjectUnavailable` | A selected object was taken by someone else |
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
- **Status entry point.** Setting the status of an `idx` range updates `status` and `visual` and grows the dynamic buffer's dirty range; `SetSnapshot` and `ApplyDelta` (§17) call it once per run of equal status.
- **Memory at 100,000 seats:** 18 bytes per seat, 1.8 MB, for these arrays, plus the chart's tables (about 0.2 MB for the synthetic 100,000-seat stadium, measured locally). Loading also holds the 16 bytes per seat of buffer data being uploaded until the upload returns.

Derived at load:

- Per-section and per-category availability counters, updated incrementally with every status change (with state application, §17).
- A neighbour table is not needed: seats within a row are stored in physical order, and `RSEG` marks the breaks.

## 6. GPU resources

| Resource | Size | Updated |
|---|---|---|
| Seat static buffer | 12 bytes × seats | Once |
| Seat dynamic buffer | 4 bytes × seats | Sub-ranges on state change |
| Decoration mesh (vertices, indices) | From `MESH` | Once |
| Table and booth instance buffer | 24 bytes × objects | Once; dynamic part on state change |
| Palette texture | 24 × (categories + 1), RGBA8 | On theme or pricing change |
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
| 4 | Section fills | `mesh` | Indexed triangles | Overview and cross-fade (§10) |
| 5 | Area fills | `mesh` | Indexed triangles | Always |
| 6 | Tables and booths | `shape` | Instanced quad | Seats and detail levels |
| 7 | Seats | `shape` | Instanced quad, one or more ranges | Seats and detail levels |
| 8 | Seat icons (accessible, selected tick, unavailable cross) | `glyph` | Instanced quad | Detail level |
| 9 | Seat labels | `glyph` | Instanced quad | Detail level |
| 10 | Row labels, section labels, text objects, area labels | `glyph` or `text` | Instanced quad | By size |
| 11 | Decoration, upper layer and icons | `mesh`, `glyph` | — | Always |
| 12 | Overlays: hover ring, focus ring, selection box | `shape` | Instanced quad | When present |

Programs:

| Program | Vertex inputs | Fragment work |
|---|---|---|
| `shape` | Corner from vertex id; per-instance position, size, angle, shape, category, visual, fx | Signed-distance shape, fill, stroke, anti-aliasing |
| `mesh` | Position, colour | Flat colour × layer alpha |
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
- Stroke is drawn where `−stroke_px < d × half_px < 0`. Below 5 px on screen the stroke is skipped.
- `fx` bit 0 (dim) multiplies alpha by the theme's dim factor. Bit 2 (pulse) blends a short highlight driven by a time uniform.
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

Picker mode: the buyer's own selection wins over the status (pending 2, held 3); otherwise status 0 is available and any other status unavailable. Filters do not change the state; they set the dim bit of `fx`. Manager mode keeps codes 4–7; their colours are placeholders until that mode is built.

The palette texture has one row per category and one extra, last row for objects without a category. Each row has three texels per visual state: fill, stroke, ink. A state whose colour does not depend on the category is written identically into every row, so the shader always indexes by `cat`. Changing a theme or a category colour rewrites the texture and touches no instance data.

| State | Fill | Stroke | Ink |
|---|---|---|---|
| Available, custom | Category colour | `seat-available-stroke` | Black or white, whichever contrasts more with the fill |
| Unavailable, booked | `seat-unavailable-fill` | `seat-unavailable-stroke` | `fg-muted` |
| Selected (pending, held) | `seat-selected-fill` | `seat-selected-fill` | `seat-selected-ink` |
| Blocked | `fg-muted` | `fg` | `bg` |
| Held by another | Category colour | `fg` | As available |

States must differ by more than colour (`11-accessibility.md` §6): unavailable seats are drawn hollow or with a cross at detail level, selected seats carry a tick.

## 10. Level of detail

Let `s` be the seat size in CSS pixels at the current zoom.

| Level | Condition | Drawn |
|---|---|---|
| **Overview** | `s < 2.5` and the chart has sections | Section fills tinted by availability, section labels, "from" price; no seats |
| **Cross-fade** | `2.5 ≤ s < 5` | Section fills fade out while seats fade in |
| **Seats** | `5 ≤ s < 14` | Flat seats, no stroke, no seat labels; row labels from `s ≥ 8` |
| **Detail** | `s ≥ 14` | Stroke, seat labels, icons |

- A chart without sections skips overview and draws seats as dots at any size.
- Thresholds have 10% hysteresis so that the level does not flicker at a boundary.
- Section tint: the section's dominant category colour, desaturated in proportion to the share of unavailable seats; a sold-out section is neutral grey with a "sold out" label.
- Tapping a section in overview flies the camera to that section.

## 11. Culling and draw calls

- Chunks hold at most 4,096 seats and have a bounding box. A chunk is drawn if its box intersects the view.
- Visible chunks that are adjacent in `idx` order are merged into one instance range.
- At overview and when the whole chart is visible, all seats are one range and one draw call.
- Target: at most 64 seat draw calls in any frame.
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
2. Find sections whose bounding box, grown by the tolerance, contains the point (a short linear scan; sections are few).
3. Within those, find rows whose bounding box contains the point.
4. Within those rows, find the seat with the nearest centre.
5. Accept it if the distance is within the tolerance.

- Tolerance: the larger of the seat's half size and 12 CSS px (touch) or 4 CSS px (mouse).
- Tables, booths and areas are tested against their shapes after seats.
- **Zoom-to-select:** on touch, if `s < 18` the tap does not select. It flies the camera to make seats 32 px, centred on the tap. With a mouse, selection works from `s ≥ 5`.
- Target: under 20 µs per hit test at 100,000 seats.

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

- Rules run locally first (`02-domain-model.md` §12) using the same code the server uses.
- Hold requests for one token are sent in order, one in flight at a time; taps made meanwhile are coalesced into the next request.
- **Group selection:** with a group size *n*, one tap selects the *n* seats in the tapped seat's segment that contain it and create the fewest orphans, preferring the window centred on the tap.
- `session: "manual"` skips hold requests; the host holds at checkout.
- If a delta makes a selected object unavailable, it is removed from the selection and `ObjectUnavailable` fires.
- Deselecting sends a release.

## 17. Applying state

- **Snapshot:** one pass computes `visual` for every object and uploads the dynamic buffer once.
- **Delta:** changed objects update `status`, `visual`, the section counters and a dirty range. Dirty ranges are merged and uploaded once per frame.
- **Private holds:** objects held under the buyer's own token are restored as selected-held after a reload (`restoreHold`).
- **Pulse:** objects that just became unavailable set the pulse bit for 400 ms, unless reduced motion is requested.
- Sequence gaps trigger a resync through the adapter; the engine never guesses.

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
- Selection ring, hover ring, focus ring.
- Section tint strength, dim factor.
- Font family for dynamic text.

Built-in themes: `light`, `dark`, `high-contrast`, and a `colorblind` modifier that switches to a tested palette and stronger shape cues. Category colours come from the chart and can be overridden by the host.

- Until `SetTheme` is implemented (REN-041) the engine uses the light theme. It sets the palette (§9), the clear colour (`bg`) and the dim factor. Its unavailable seats are filled close to the background with a strong stroke, so they read as hollow and differ from solid available seats by more than hue (`11-accessibility.md` §6); the stroke has 4.6:1 contrast (WCAG) against the background, and below 5 px on screen, where there is no stroke, the difference is lightness. The synthetic charts' category colours have at least 4.5:1 against the background.

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
| Meshes | ≤ 2 MB |
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
