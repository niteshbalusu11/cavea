# TODO

**Status:** Draft v0.1 · 2026-11-10

Every task, grouped by milestone (`ROADMAP.md`). Each item names the spec section it implements, for example `(04 §10)` means `04-renderer.md` section 10.

Prefixes: `VAL` validation and business · `FND` foundation · `FMT` chart format and compiler · `REN` renderer · `WEB` web SDK · `ENG` seat engine · `SRV` server · `RT` realtime · `DSN` designer · `CON` console · `MOB` mobile · `AI` import · `A11Y` accessibility · `SEC` security · `LIC` licensing · `TST` testing · `DOC` documentation · `OPS` build and release.

Do not start a milestone until the previous one's exit criteria are met.

---

## M0 · Validation spike

### Business

- [ ] **VAL-001** Read the employment contract's intellectual-property and outside-work clauses; get written confirmation if unclear (14 §9)
- [ ] **VAL-002** Choose a working name; check trademarks, domain, npm scope, crate names, GitHub organisation (14 §9)
- [ ] **VAL-003** Build the prospect list: 40 ticketing platforms with vendor, size, country and a named technical contact (14 §1.2)
- [ ] **VAL-004** Write the two outreach variants and the interview template (14 §1.3, §1.4)
- [ ] **VAL-005** Build the landing page: positioning, three wedges, pricing hypothesis, early-access form (14 §1.1)
- [ ] **VAL-006** Deploy the public demo page on static hosting (14 §1.1)
- [ ] **VAL-007** Record a 30-second video of the demo on a mid-range phone (14 §1.1)
- [ ] **VAL-008** Send 20–30 messages; log every reply (14 §1.3)
- [ ] **VAL-009** Hold at least five interviews; record answers verbatim (14 §1.4)
- [ ] **VAL-010** Write the go, reposition or stop review (14 §1.5, 00 §9)

### Foundation

- [x] **FND-001** Create the monorepo skeleton: cargo workspace, bun workspace, directory layout, `CLAUDE.md` and `AGENTS.md` (01 §3)
- [x] **FND-002** Basic CI with caching: `cargo fmt`, `clippy`, tests, WASM build; oxc lint and format, strict `tsc` (01 §4, 13 §12)

### Rendering spike

- [x] **FMT-001** Synthetic stadium generator emitting raw position and category arrays (03 §7)
- [x] **REN-001** Spike: draw 100,000 instanced seats with WebGL2 from Rust through `glow` (04 §7, §8)
- [x] **REN-002** Spike: translate WGSL to GLSL ES 3.00 in `build.rs` with `naga`; record whether it works for the seat shader (DECISIONS D-05)
- [x] **REN-003** Spike: measure the WASM and JavaScript size of the minimal renderer after `wasm-opt` and brotli; record it (01 §5)
- [x] **REN-004** Spike: pan, pinch and wheel zoom with inertia (04 §14)
- [x] **REN-005** Spike: hit test and tap-to-select with local state (04 §15)
- [x] **REN-006** Spike: frame-time meter and timing marks on the page (04 §24)
- [ ] **TST-001** Choose and buy the reference devices (01 §5)
- [ ] **TST-002** Record spike frame times and load times on each reference device (13 §6.2)

---

## M1 · Core, format, web renderer

### Foundation

- [x] **FND-003** Scaffold crates `cavea-core`, `cavea-format`, `cavea-compiler`, `cavea-render`, `cavea-web`, `cavea-cli`, `cavea-testkit` with the dependency rule enforced (01 §2, §3)
- [x] **FND-004** `cargo xtask build-web`: build, `wasm-opt`, brotli, emit size report (01 §6)
- [x] **FND-005** Payload size gate in CI with a hard cap and a +3% warning (13 §6.3): `cargo xtask build-web` fails above +3% over the committed baseline
- [x] **FND-006** Release profile tuning for WASM: LTO, `panic = "abort"`, optimisation level, allocator choice; measure each (01 §5)
- [ ] **FND-007** Generate TypeScript types from Rust for options, events and payloads (05 §3, §5)
- [x] **FND-008** Workspace lint configuration: moderate clippy set, strict `tsc`, oxc rules (01 §4)
- [x] **FND-009** Changelog tooling and commit conventions (01 §6)
- [x] **FND-010** Nix development environment: `flake.nix` with every non-trivial tool, `.envrc` using the flake, so `direnv allow` loads everything (ORCHESTRATION §5, §10)

### Chart format and compiler

- [x] **FMT-002** Chart document types with serde; top-level, settings, categories, floors (03 §2.1)
- [x] **FMT-003** Node types and common fields (03 §2.2)
- [x] **FMT-004** Row label scheme engine: alpha, lower, numeric, roman, custom, skip, direction, overflow (03 §2.5)
- [x] **FMT-005** Seat label scheme engine: numeric, alpha, odd, even, custom; ltr, rtl, centre-out, serpentine; continuous (03 §2.5)
- [x] **FMT-006** Expand straight row blocks: pitch, alignment, offset (03 §2.4)
- [x] **FMT-007** Arc and bulge curves (03 §2.4)
- [x] **FMT-008** Stagger, skew, removed seats, per-seat overrides, `oid` runs (03 §2.4)
- [x] **FMT-009** Free-form single row along a polyline or curve (03 §2.6)
- [x] **FMT-010** Sections: child transforms, automatic outline as concave hull (03 §2.3)
- [x] **FMT-011** Row segmentation at gaps and removed seats (03 §3.1 step 5)
- [x] **FMT-012** Label templating and uniqueness check (03 §3.1 steps 6–7)
- [x] **FMT-013** Validation errors E001–E010 with node ids (03 §3.2); E004 waits for areas
- [x] **FMT-014** Validation warnings W101–W110 (03 §3.2)
- [x] **FMT-015** Seat rank computation, versioned (03 §3.3)
- [x] **FMT-016** `idx` ordering and chunking to at most 4,096 seats (03 §3.1 steps 9–10)
- [x] **FMT-017** Section label anchor by pole of inaccessibility and maximum font size (03 §3.1 step 11)
- [x] **FMT-018** Tessellate shapes, outlines and table shapes with `lyon` (03 §3.1 step 12); section outlines and shapes done (strokes are built without `lyon`, D-25); table shapes wait for tables
- [x] **FMT-019** Decoration nodes: shape, text, icon, image (03 §2.7)
- [x] **FMT-020** CCF writer: header, directory, all chunks and encodings (03 §4.1, §4.2)
- [x] **FMT-021** CCF reader: validated, single pass, no panics (03 §4.4)
- [x] **FMT-022** Emit public and full artefacts with content hash (03 §3.1 steps 14–15)
- [x] **FMT-023** Schema migration framework with stored samples (03 §2.8)
- [x] **FMT-024** CLI: `cavea compile`, `validate`, `inspect`, `gen` (01 §2)
- [x] **FMT-025** All synthetic presets, seeded (03 §7). `banquet` and `festival` wait for tables, booths and areas
- [x] **FMT-026** Measure compiled sizes against targets; record (03 §4.5)

### Renderer engine

- [x] **REN-010** Engine command and event types (04 §3)
- [x] **REN-011** Host service traits: clock, frame scheduler, text rasteriser, image source (04 §2)
- [x] **REN-012** Scene arrays loaded from CCF; per-row, segment, section, chunk tables (04 §5)
- [x] **REN-013** `GpuBackend` trait and fixed pipeline enum (04 §20.1)
- [x] **REN-014** `glow` backend for WebGL2 with the specified context attributes (04 §20.2)
- [x] **REN-015** Shader build pipeline: WGSL sources, generated GLSL, or the hand-written fallback (DECISIONS D-05)
- [x] **REN-016** `shape` program: instanced quads, signed-distance shapes, stroke, anti-aliasing (04 §8)
- [x] **REN-017** Seat static and dynamic buffers with the specified layout (04 §6)
- [x] **REN-018** Palette texture and visual state mapping (04 §9)
- [x] **REN-019** `mesh` program: decoration layers and section fills (04 §7)
- [ ] **REN-020** `image` program and lazy background image loading (04 §7)
- [x] **REN-021** Level-of-detail levels with cross-fade and hysteresis (04 §10)
- [x] **REN-022** Section availability counters and overview tint (04 §5, §10)
- [x] **REN-023** Chunk culling and merging of visible ranges; stay under 64 seat draw calls (04 §11)
- [x] **REN-024** MSDF atlas bake tool and `glyph` program (04 §12)
- [x] **REN-025** Seat labels from row rules, built per chunk with an LRU of 64 (04 §12)
- [x] **REN-026** Dynamic text atlas, size buckets, `text` program, browser rasteriser (04 §12)
- [x] **REN-027** Row labels, section labels, text objects with greedy collision (04 §12)
- [x] **REN-028** Icon atlas and seat icons: accessible, tick, cross (04 §7). Signed-distance strokes in the `shape` program instead of an atlas (D-42)
- [x] **REN-029** Camera: fit, limits, clamping, `FlyTo` in log-zoom space (04 §13)
- [x] **REN-030** Gesture recogniser: drag, pinch, wheel, trackpad, double tap, two-finger tap, long press, inertia (04 §14)
- [x] **REN-031** Gesture handling modes and `touch-action` (04 §14)
- [x] **REN-032** Hierarchical hit testing with tolerance (04 §15)
- [x] **REN-033** Zoom-to-select on touch (04 §15)
- [x] **REN-034** Selection engine: optimistic select, hold request and result, revert (04 §16)
- [x] **REN-035** Maximum and minimum selection rules (02 §12)
- [x] **REN-036** Snapshot application in one pass (04 §17)
- [x] **REN-037** Delta application with merged dirty ranges (04 §17)
- [x] **REN-038** On-demand frame scheduling with dirty flags and an animation list (04 §18)
- [x] **REN-039** Resize and device-pixel-ratio handling with caps (04 §21)
- [x] **REN-040** Context loss and restoration (04 §20.2)
- [x] **REN-041** Theme tokens to palette; light, dark and high-contrast themes (04 §19)
- [x] **REN-042** Overlay pass: hover ring and focus ring (04 §7)
- [x] **REN-043** Pulse on change, and reduced-motion handling (04 §17, §13)
- [x] **REN-044** Timing marks and frame statistics (04 §24)
- [ ] **REN-045** Headless render to a pixel buffer for golden tests (04 §26)

### Web SDK

- [x] **WEB-001** Package scaffold; loader that starts WASM, chart, state and atlas fetches together (04 §4, 05 §1). The script-tag build waits for WEB-013
- [x] **WEB-002** `createSeatMap` with options validation and development warnings (05 §3). Options of unbuilt features are typed out and warned about; the capability ladder waits for REN-071
- [x] **WEB-003** `StateAdapter` interface, `staticState`, `customState` (05 §4)
- [x] **WEB-004** Callbacks and the `SeatObject` payload (05 §5). `onHoldExpiring` and `onHoldExpired` wait for WEB-024; `onSectionEnter`, `onFloorChange`, `onMetrics` and `price`/`ticketType` (WEB-009) for their features
- [x] **WEB-005** Methods: selection, pricing, theme, zoom, find, list, destroy (05 §6). `setPricing` waits for WEB-009; floors as zoom targets, per-category availability and the other unbuilt methods of 05 §6 for their features
- [x] **WEB-006** Shadow-root interface: zoom controls, legend, loading and error states (05 §7)
- [x] **WEB-007** Tooltip with price, ticket-type choice and flags; host replacement (05 §7)
- [x] **WEB-008** Toasts for refusals and rule violations (05 §7)
- [x] **WEB-009** Pricing display and `formatPrice` (05 §3.1)
- [x] **WEB-010** Theme tokens as CSS custom properties, applied live (05 §8)
- [x] **WEB-011** Message catalogue in English, `messages` override, `locale` (05 §3.4)
- [x] **WEB-012** `preloadSeatMap` and preload documentation (05 §10)
- [x] **WEB-013** Script-tag build (05 §1)
- [x] **WEB-014** React wrapper with one-to-one props and imperative ref (05 §11)
- [x] **WEB-015** Server-side-rendering safety test with Next.js (05 §10)
- [x] **WEB-016** Content Security Policy test page and documentation (05 §10)
- [x] **WEB-017** Error model and degradation behaviours (05 §9)
- [x] **WEB-018** Bundler compatibility checks: Vite, webpack 5, Rollup, esbuild (13 §8)

### Accessibility baseline

- [x] **A11Y-001** Map as one tab stop with arrow-key virtual focus and Enter to select (11 §3)
- [x] **A11Y-002** Hidden proxy element with rebuilt accessible name; polite live region for selections (11 §5)

### Testing

- [ ] **TST-010** Golden-image harness with tolerance and stored baselines (13 §6.1)
- [x] **TST-011** Micro-benchmarks: decode, hit test, delta apply, label build (13 §6.3)
- [ ] **TST-012** Benchmark page with the scripted gesture trace and result collector (13 §6.2)
- [x] **TST-013** Fuzz targets `ccf_decode` and `document_parse` with seed corpora (13 §5)
- [x] **TST-014** Compiler round-trip and determinism tests across platforms (13 §4)
- [ ] **TST-015** Scripted interaction tests producing expected engine events (13 §6.1)
- [ ] **TST-016** Run S1–S5 on the reference devices; commit results (13 §6.2)

---

## M2 · Server and live state

### Seat engine

- [x] **ENG-001** Domain types: statuses, holds, commands, replies, failure reasons (02 §4, §5)
- [x] **ENG-002** `SeatStore` trait and an in-memory implementation with fault injection (08 §5)
- [x] **ENG-003** Event actor state and mailbox (08 §4.1)
- [x] **ENG-004** Actor loop with natural batching and acknowledge-after-commit (08 §4.3)
- [x] **ENG-005** Commands: create hold, hold, release, extend (08 §4.2, §7.1)
- [x] **ENG-006** Commands: book with token, book direct, change status (08 §7.2)
- [x] **ENG-007** All-or-nothing validation with per-object failure reasons (02 §4.3)
- [x] **ENG-008** Hold expiry heap and internal expire command (08 §7.3)
- [x] **ENG-009** Idempotency: in-memory window plus stored records (08 §6.1)
- [x] **ENG-010** Public snapshot re-encoding and lock-free publication (08 §4.5)
- [x] **ENG-011** Delta coalescing and the 60-second ring buffer (08 §8.3)
- [x] **ENG-012** Ownership: acquire, epoch, lease renewal, fencing in commit (08 §4.7, §4.8)
- [x] **ENG-013** Load from storage, including overdue expiries; idle eviction (08 §4.7)
- [x] **ENG-014** Poison and reload on commit failure or panic (08 §4.6)
- [x] **ENG-015** Mailbox bounds and overload replies (08 §4.3)
- [x] **ENG-016** Actor registry with concurrent load de-duplication (08 §3)

### Server

- [x] **SRV-001** Server scaffold: axum, configuration, tracing, request ids (08 §2, §3, §13)
- [x] **SRV-025** `cavea-telemetry` crate: structured logging setup, HTTP request spans with request id, child spans for mailbox wait, apply and commit (AGENTS.md Logging, 08 §15)
- [x] **SRV-002** Schema and embedded migrations under an advisory lock (08 §5)
- [x] **SRV-003** PostgreSQL store: pipelined batch commit transaction (08 §4.4)
- [x] **SRV-004** API keys: generation, hashing, prefixes, extractors (08 §11)
- [x] **SRV-005** Workspaces and admin-key bootstrap (08 §11)
- [x] **SRV-006** Problem+json errors with the stable code table (08 §6.4)
- [x] **SRV-007** Chart endpoints: create, draft with etag, validate, publish, versions (08 §6.2)
- [x] **SRV-008** Compile on publish; store public, precompressed and full artefacts (03 §4.6)
- [x] **SRV-009** Serve compiled charts with immutable caching and content negotiation (03 §4.6)
- [x] **SRV-010** Asset upload and serving (08 §6.2). Upload, sniffing and public serving (D-49), image bytes in memory bounded per node, E007 at validate and publish for an image the workspace does not hold; attaching a thumbnail to a version is DSN-040
- [x] **SRV-011** Event endpoints: create, get, patch, archive (08 §6.2)
- [x] **SRV-012** Hold-token endpoints (08 §6.2)
- [x] **SRV-013** Seat operation endpoints: hold, release, book, change status (08 §6.2)
- [x] **SRV-014** Objects query endpoint (08 §6.2)
- [x] **SRV-015** Public endpoints with CORS and origin allow-list (08 §6.2, §11). Done for the event, hold-token, hold and release routes; `state`, `stream`, `rt`, `best-available`, `sections`, `rows` and `assets` come with realtime (RT), best-available, the list view and asset upload
- [x] **SRV-016** Session token verification and `require_session` (08 §11)
- [x] **SRV-017** Idempotency-Key handling and mismatch detection (08 §6.1)
- [x] **SRV-018** Rate limiting per key and IP with trusted-proxy handling (08 §12)
- [x] **SRV-019** OpenAPI document generated and served (08 §6.1). From the route table that registers the routes (D-46); committed as `crates/cavea-server/openapi.json`; the router tests validate every response against it
- [x] **SRV-020** Health, readiness and Prometheus metrics (08 §15). `/readyz` follows a once-a-second database probe and shutdown; `/metrics` takes the admin key; every §15 metric but the webhook ones, which come with SRV-032, plus mailbox wait, fan-out time, frames, rate-limit refusals, pool connections and dropped log records
- [x] **SRV-021** Graceful shutdown: flush, close subscribers, release leases (08 §14)
- [x] **SRV-022** Startup load of events with outstanding holds (08 §4.7)
- [x] **SRV-023** Dockerfile (distroless, multi-arch) and compose file (08 §14). `deploy/Dockerfile`, `deploy/compose.server.yaml`, `deploy/smoke.ts` (D-63). The Helm chart and systemd unit 08 §14 lists are not done
- [x] **SRV-024** Body size, object count and JSON depth limits (12 §3 T19)

### Realtime

- [x] **RT-001** Frame codecs in `cavea-format`, shared by server and clients (08 §8.1, §8.2)
- [x] **RT-002** Snapshot encodings: raw, run-length, bitset; pick the smallest (08 §8.1)
- [x] **RT-003** WebSocket endpoint: subscribe, hello, resume from `last_seq` (08 §8.3)
- [x] **RT-004** Fan-out of shared pre-encoded frames (08 §4.5)
- [x] **RT-005** Slow-consumer detection and RESYNC (08 §8.3)
- [x] **RT-006** Ping, pong and idle timeouts (08 §8.3)
- [x] **RT-007** SSE transport (08 §8.4)
- [x] **RT-008** Polling with `after_seq` (08 §8.4)
- [x] **RT-009** HTTP snapshot with ETag and cache headers (08 §8.5)
- [x] **RT-010** PRIVATE frames for the caller's holds (08 §8.1)

### Web SDK

- [x] **WEB-020** `caveaServer` adapter: binary snapshot, transport auto-selection, resync (05 §4). Reloading the chart in place after CLOSE 2 waits for event upgrades (ENG-029)
- [x] **WEB-021** Hold-on-select: ordered single-flight requests with coalescing (04 §16)
- [x] **WEB-022** Session modes: start, continue, manual, none (05 §4). `none` selects without holding, as `manual`, until `mode: 'static'` exists
- [x] **WEB-023** Hold token persistence and restore after reload (05 §4)
- [x] **WEB-024** Expiry callbacks and the optional hold timer (05 §5, §7). The callbacks and the timer's data (`expiresAt` on the page's clock); the countdown control waits for the built-in interface (WEB-006)
- [x] **WEB-025** Pause when hidden and resync on return (05 §4)
- [ ] **WEB-026** `@cavea/server-sdk` for Node, generated from OpenAPI with an ergonomic wrapper (05 §1)

### Testing

- [x] **TST-020** Reference model and model-based property tests for the engine (13 §3.1)
- [x] **TST-021** Deterministic simulation harness with the fault table (13 §3.2)
- [x] **TST-022** Reconciler: `cavea-server verify` (13 §3.3)
- [x] **TST-023** Load generator `cavea-loadgen` (13 §7.1)
- [ ] **TST-024** Scenarios L1 and L2 on reference hardware; record results (13 §7.2). Run locally at full scale (13 §7.4), not passed: L1 correct, hold p99 18.4 ms with 55,000 subscribers once fan-out stopped delaying requests (D-45; it was 1.13 to 1.31 s), 15.1 to 16.1 ms with the 5,000 buyers alone, so the 3.4 ms over the budget is this desktop's commit latency and lookups, not subscribers; delta delivery p95 1.1 s on loopback (not an L1 criterion). L2 correct for 600 s with memory flat, but hold p99 23 ms against a private database (commit latency on desktop storage; 15.1 ms in a 100 s run after D-45) and stalls from the host's connection-tracking table. Reference-hardware runs remain for the owner
- [x] **TST-025** Chaos: `kill -9` and PostgreSQL restart during load (13 §7.3). Passed locally (13 §7.4): 6 kills and 6 PostgreSQL restarts or kills under load, reconciler clean, every acknowledgement present, automatic recovery; a `500`-for-`503` defect fixed, and the log output that stopped the server when it failed (13 §7.4). Reference-hardware runs remain for the owner
- [x] **TST-026** Authorisation matrix and tenant isolation tests (13 §10). Every route of the route table × every credential kind, with session and hold-token columns where they apply
- [ ] **TST-027** Fuzz targets `wire_decode`, `snapshot_apply` and HTTP (13 §5). `wire_decode` and `snapshot_apply` exist (`fuzz/`); the HTTP target waits for the OpenAPI document

---

## M3 · Designer MVP

### Edit core

- [x] **DSN-001** `cavea-edit` crate: document arena and node types (07 §2)
- [x] **DSN-002** Command framework with computed inverses (07 §2)
- [x] **DSN-003** Undo and redo stack with coalescing for drags and scrubbing (07 §9)
- [x] **DSN-004** Live scene: incremental expansion per changed node (04 §25). Each node, section child and section shell compiled alone and merged into one chart the engine draws (D-55); `Canvas` in the designer module and `@cavea/designer-core`, with a proof page (`packages/designer-core/proof`)
- [x] **DSN-005** Renderer extensions: chunk transforms and the overlay batcher (04 §25). Pieces moved in a gesture drawn with a matrix and baked after it (D-56); the overlay tessellated on the CPU for the `mesh` and `text` programs (D-57); section outlines cached in the edit core (07 §14)
- [x] **DSN-006** View models and their generated TypeScript types; throttled to one per frame (07 §2, §14). `Session::changed` once per frame (D-40); the module `cavea-designer`, `@cavea/designer-core` and the types generated by `cargo xtask types` (07 §2.4, D-41). The shell runs the frame loop (DSN-008)
- [x] **DSN-007** Spatial index for snapping and selection (07 §5.2)

### Shell

- [x] **DSN-008** Application shell: top bar, tool rail, canvas, inspector, panels, status bar; resizable and persistent (07 §3). `apps/console/src/designer`: the layout, the frame loop and its stores, the canvas region behind `CanvasHost` (camera, input conversion; drawing is DSN-004, DSN-005), persisted layout (07 §3, the shell). Floor add, rename and delete are DSN-053
- [x] **DSN-009** Command palette and shortcut help (07 §8). Built in the shell (`Palette.tsx`, `commands.ts`, `Help.tsx`, 07 §3); searching charts and events from the palette (09 §8) waits for the console's server screens
- [x] **DSN-010** All keyboard shortcuts, with macOS variants (07 §8). One table, `apps/console/src/designer/shortcuts.ts`; the rows whose actions do not exist yet are in it as pending with their tasks (group, rulers, palette, the table, booth, area, image and measure tools), and each of those tasks binds its row

### Tools

- [x] **DSN-011** Select tool: click, shift-click, box, enter and exit nested levels (07 §5.1)
- [x] **DSN-012** Move, nudge, rotate with pivot, resize semantics for blocks and shapes (07 §5.1)
- [ ] **DSN-013** Hand tool, zoom shortcuts, zoom to selection (07 §4). The edit core is built: `ToolId::Hand`, Space held, the middle button, `pan` in `InputResult`, the boxes `selection_view().bounds` and `fit()` (07 §2.3, §4); the shell and renderer owe the camera, the zoom shortcuts and framing those boxes
- [x] **DSN-014** Row block tool with live counts and numeric entry (07 §4)
- [x] **DSN-015** Arc block tool (07 §4)
- [x] **DSN-016** Single row tool with optional bend (07 §4)
- [x] **DSN-017** Section tool and automatic outline; convert to polygon (07 §5.5)
- [x] **DSN-018** Shape tools: rectangle, ellipse, polygon, line (07 §4)
- [x] **DSN-019** Text tool (07 §4)
- [ ] **DSN-020** Image tool: upload, reference or background role, opacity, lock (07 §4)
- [x] **DSN-021** Focal point tool (07 §4)
- [x] **DSN-022** Category brush (07 §4)

### Panels and editing

- [ ] **DSN-023** Inspector for row blocks with the curve handle (07 §5.3). The edit core is built: `Session::inspector`, `set_field`, scrubbing, the select tool's curve handle (07 §2.3, §5.3); the shell owes the inspector and drawing the handle; the inspector is built (`Inspector.tsx`: fields by kind, mixed values, scrubbing, the curve fields), and drawing the handle waits for the canvas (DSN-004, DSN-005)
- [x] **DSN-024** Inspector for sections, shapes, text, images and chart settings (07 §5.5). The edit core is built: the `section`, `shape`, `text`, `image` and `chart` groups of `Session::inspector` (07 §2.3); a section's label position waits for a schema change (07 §5.5); the shell's inspector is built (`Inspector.tsx`)
- [x] **DSN-025** Row-level editing: add or remove seats, shift, insert aisle, override label, delete row (07 §5.3)
- [x] **DSN-026** Seat-edit mode: remove, restore, flags, category, label, nudge (07 §5.3)
- [ ] **DSN-027** Numbering panel for rows and seats with canvas preview (07 §6). The edit core is built: `Session::numbering`, `preview_numbering`, `number`, `sample` (07 §2.3, §6); the panel is built (`NumberingPanel.tsx`, 07 §6 "In the shell"); drawing the preview's labels on the canvas, and the hovered seat's sample, wait for the canvas (DSN-004, DSN-005)
- [x] **DSN-028** Categories panel: add, rename, recolour, merge, delete with reassignment; contrast warning (07 §7). The edit core is built: `Session::categories` and the category calls, `contrast`, `too_similar` (07 §2.3, §7); the shell owes the panel and the colour picker; the panel is built (`CategoriesPanel.tsx`: the list, the active category, add, edit with the live W107 checks, reorder by drag or Alt+↑/↓, merge, delete with reassignment)
- [x] **DSN-029** Layers panel: virtualised tree, drag to reparent, lock, hide (07 §3). The edit core is built: `Session::layers` by page, `expand_layer`, `reveal_layer`, `click_layer`, `move_layers`, `lock_layer`, `hide_layer`, and the selected ids in `selection_view` (07 §2.3); the panel is built (`LayersPanel.tsx`: virtual tree by page, keyboard, drag and drop of the whole selection, context menu, revealing a selection made elsewhere; 07 §3)
- [x] **DSN-030** Snapping: grid, edges and centres, equal spacing, row alignment, angles (07 §5.2)
- [ ] **DSN-031** Rulers and user guides (07 §5.2). The edit core is built: guides per floor as snap targets, `Session::add_guide` and the rest, `saved_guides` (07 §5.2); the shell owes the rulers, dragging guides out of them, and storing the guides beside the draft
- [x] **DSN-032** Align, distribute, flip, duplicate, array duplicate (07 §5.1)
- [x] **DSN-033** Copy and paste, including between charts (07 §5.1)
- [ ] **DSN-034** Group, ungroup, order for decoration nodes (07 §5.1). Order, lock and hide are built; grouping needs a schema change (07 §5.1, group and ungroup)

### Saving, validation, publishing

- [ ] **DSN-035** Autosave with etag and save-state indicator (07 §9)
- [ ] **DSN-036** IndexedDB safety net and recovery prompt (07 §9)
- [ ] **DSN-037** Single-editor lock with take-over (07 §9)
- [ ] **DSN-038** Continuous validation, panel, canvas markers (07 §10). The edit core is built: `Session::validate`, `validation`, `focus`, the overlay's `markers` (07 §2.3, §10); the shell owes the idle-time loop, the panel, the status-bar summary, the markers on the canvas and disabling publish; the shell's idle-time loop, the status-bar summary and disabling Publish are built (`loop.ts`, `Rails.tsx`, `TopBar.tsx`); the panel is built (`ValidationPanel.tsx`: a virtual list, errors then warnings under headings with their counts, stale marked, a finding focuses and reveals its node); the markers wait for the canvas (DSN-004, DSN-005)
- [ ] **DSN-039** Preview with the buyer renderer, device widths and sample states (07 §11)
- [ ] **DSN-040** Publish flow: compile, diff, thumbnail upload (07 §11)
- [x] **DSN-041** Export and import of chart document JSON (07 §12). The edit core is built: `Session::export`, `Session::import` with typed `ImportError`s (07 §12); the shell's menu items, file picker and drop, download, and the confirmation before replacing a chart are built (07 §12, in the shell)
- [ ] **DSN-042** New-chart screen, three templates, first-run tour (07 §16). The new-chart screen and the three templates are on the unreviewed branch `slice/designer-shell-2-insert-new`; the five-step tour is about canvas gestures and waits for the canvas (DSN-004)
- [ ] **DSN-043** "Insert row block" form as the non-pointer path (07 §15). The form is on the unreviewed branch `slice/designer-shell-2-insert-new`
- [ ] **DSN-044** Performance pass on the 100,000-seat chart (07 §14)

### Testing

- [x] **TST-030** Command inverse property tests (13 §9)
- [ ] **TST-031** End-to-end: draw, number, publish, create event, hold a seat (13 §9)
- [ ] **TST-032** Usability test: three new users draw the reference theatre; record times (ROADMAP M3)

---

## M4 · Console, licensing, docs → private beta

### Console

- [x] **CON-001** Application shell, router, navigation, workspace switcher (09 §1). `apps/console`: router (under Vite's `base`), the console frame and its navigation, themes, tokens, the message catalogue, base components; the workspace switcher (hidden with one workspace), the signed-in user's menu, every frame route behind the session, and the generated API client with its request layer (09 §9 as built, D-60)
- [x] **CON-002** First-run setup with a one-time link (09 §2). The logged link, `POST /console/api/setup`, one owner under a race, owner of every workspace (D-51); the `/console/setup` page takes the token from the fragment at start, removes it from the address bar, and lands the owner signed in
- [ ] **CON-003** Sign-in, sessions, CSRF, optional TOTP (09 §2, §10). The server side is built but TOTP: argon2id, `/console/api/session`, `me`, `password`, the `__Host-` cookie with idle and absolute ends, CSRF and origin checks on every cookie mutation (08 §11, D-50). The console's screens are built: sign-in (one message for a wrong email or password, `Retry-After` waited out), sign-out, the session from `me` at start, a lost session sending the person to sign-in and back, the password change; the CSRF header and the dev proxy (09 §9). Owed: TOTP (`POST /console/api/totp`, on the server and its screens; SHA-1 HMAC is in the tree) and password reset
- [x] **CON-004** Roles and permission checks on every route (09 §2). The server side: every secret-key route takes a console session, names its workspace in `Cavea-Workspace` and declares one of twelve permissions, one per cell of 09 §2, checked against the five roles by the authorisation matrix (08 §11, D-54); ownership transfer is the members routes' `ownership`. The console shows only the sections and actions the role allows (from `me`'s permissions against each operation's `x-cavea-permission`; a test holds it to the document's roles), says why a disabled action is disabled, and shows a server's `403` `role` and re-reads `me`
- [ ] **CON-005** Charts list with thumbnails, search, card menu, empty state (09 §3.1). Built: cards (name, key, seats and version when published, last edited, archived), search by name or key, sort by name or last edited, archived on request, empty, loading and error states, create, import from a chart file, and a card menu with export, duplicate and archive. Owed: thumbnails (a placeholder until publishing makes them, DSN-040), Open in the card menu and chart detail (the designer from the server, DSN-035; CON-006), events per chart (no list filter by chart yet), the empty state's templates and reference image (DSN-042)
- [ ] **CON-006** Chart detail: versions, events, settings (09 §3.2)
- [ ] **CON-007** Events list and create form (09 §4.1)
- [ ] **CON-008** Event manager, basic: live map, inspect, select, block, release, book (09 §4.2)
- [ ] **CON-009** Event settings tab (09 §4.3)
- [ ] **CON-010** API keys screen (09 §6)
- [ ] **CON-011** Quick-start snippets pre-filled with workspace values (09 §6)
- [ ] **CON-012** Workspace and members settings; invitations (09 §7). Built on the server: list members, change a role, remove a member (`/console/api/members`, owner and admin; only an owner makes or unmakes an owner; a workspace keeps one owner; 08 §6.2). Then invitations, the admin key's owner invitation and the workspace's name (09 §7, D-61). Owed: the console's screens (members, invitations, the invitation page, workspace settings)
- [x] **CON-013** Audit log writes for sensitive actions (09 §4.2). Written in each action's transaction; `GET /console/api/audit` (09 §7, D-61). Its screen is with the settings screens
- [ ] **CON-014** Interface standards: tables, forms, feedback, themes, keyboard (09 §8)
- [x] **CON-015** Embed the built console in the server binary with hashed assets (09 §9). `cargo xtask build-server` (D-62)

### Licensing

- [ ] **LIC-001** Licence file format and private signing tool (12 §7.2)
- [ ] **LIC-002** Server verification and behaviour by state; seat operations never blocked (12 §7.3)
- [ ] **LIC-003** Renderer watermark logic, checked after the first interactive frame (12 §7.4)
- [ ] **LIC-004** Console licence screen and banners (09 §7)
- [ ] **LIC-005** Test: expired and missing licences leave hold and book working (DECISIONS D-20)

### Security

- [ ] **SEC-001** Review the threat table against the implementation; file gaps (12 §3)
- [ ] **SEC-002** SSRF guard for outbound requests (12 §3 T8)
- [x] **SEC-003** Login rate limiting and session rotation (12 §3 T9). Per-address budget, per (account, address) and per-account back-off that never waits on an address the member signed in from (08 §12, D-51); a new session at every sign-in, every session ended by a password change
- [ ] **SEC-004** Content Security Policy and security headers for the console (12 §3 T7)
- [ ] **SEC-005** Verify all user-supplied strings render as text in renderer, list view and console (12 §3 T7)
- [ ] **SEC-006** Secure defaults check at startup with warnings (12 §4)
- [ ] **SEC-007** `SECURITY.md`, contact address, disclosure policy (12 §6)
- [ ] **SEC-008** Signed images and packages; SBOM per release (12 §3 T15)
- [ ] **SEC-009** Log scrubbing: no tokens, keys or bodies (12 §4)

### Documentation

- [ ] **DOC-001** Docs site scaffold and deployment (14 §7)
- [ ] **DOC-002** Quick start: Docker to a rendered chart in ten minutes (14 §7)
- [ ] **DOC-003** Concepts pages (14 §7)
- [ ] **DOC-004** Renderer reference: options, callbacks, methods, theming, adapters (05)
- [ ] **DOC-005** Server API reference generated from OpenAPI (08 §6)
- [ ] **DOC-006** Deployment guide: sizing, PostgreSQL, TLS and WebSocket, backups, upgrades, monitoring (08 §14)
- [ ] **DOC-007** Guides: hold, pay, book; bring your own backend; React (14 §7)
- [ ] **DOC-008** Designer user guide for venue staff (07)
- [ ] **DOC-009** Examples repository: plain page, React, Next.js, Node backend (14 §5)
- [ ] **DOC-010** Error code, limits and changelog reference (08 §6.4, 02 §14)

### Build and release

- [ ] **OPS-001** Release pipeline: multi-arch images, npm packages, signing, changelog (01 §6)
- [ ] **OPS-002** Version and compatibility policy written down (01 §6)
- [ ] **OPS-003** Upgrade test from the previous release with a populated database (13 §13)
- [ ] **OPS-004** Public demo instance with a fake checkout (14 §10)
- [ ] **OPS-005** Delivery of builds and licence files to pilots (14 §10)
- [ ] **OPS-006** Full release checklist run (13 §13)

### Business

- [ ] **VAL-020** Licence agreement drafted and reviewed by a lawyer (12 §8)
- [ ] **VAL-021** Pilot terms and success definition template (14 §4)
- [ ] **VAL-022** Pricing page with the tested prices (14 §3)
- [ ] **VAL-023** Stranger test of the quick start; fix what blocked them (ROADMAP M4)
- [ ] **VAL-024** Onboard the first pilot; draw their charts for them (14 §4)
- [ ] **VAL-025** Eight-week checkpoint against the kill criteria (00 §9)

---

## M5 · Selling features

### Seat engine

- [ ] **ENG-020** Orphan check, strict and lenient, in `cavea-core` (02 §12)
- [ ] **ENG-021** Consecutive-seat check in `cavea-core` (02 §12)
- [ ] **ENG-022** Server enforcement of the event's orphan policy on hold (08 §7.1)
- [ ] **ENG-023** Best-available: eligibility, windows, cost, pruned search (08 §7.4)
- [ ] **ENG-024** Best-available: split across rows, greedy fallback, accessible requests (08 §7.4)
- [ ] **ENG-025** General-admission areas: counters, hold, book, release by quantity (02 §8)
- [ ] **ENG-026** Tables booked whole, and booths (02 §9)
- [ ] **ENG-027** Channels: assignment, enforcement, per-channel-set public views (02 §7, 08 §8.3)
- [ ] **ENG-028** Custom statuses (02 §4.1)
- [ ] **ENG-029** Event upgrade to a new chart version with conflict report (02 §10)
- [ ] **ENG-030** Area capacity overrides per event (02 §8)

### Server

- [ ] **SRV-030** Best-available endpoints, secret and public (08 §6.2)
- [ ] **SRV-031** Channel endpoints (08 §6.2)
- [ ] **SRV-032** Webhooks: outbox rows in the commit, dispatcher, signing, retries (08 §9)
- [ ] **SRV-033** Webhook endpoints, delivery log, replay, test event (08 §6.2)
- [ ] **SRV-034** Reports: summaries and streamed CSV export (08 §10)
- [ ] **SRV-035** Log endpoint with filters (08 §6.2)
- [ ] **SRV-036** Version diff endpoint (08 §6.2)
- [ ] **SRV-037** Public section and row endpoints for the list view (08 §6.2)
- [ ] **SRV-038** Embed tokens (08 §11)
- [ ] **SRV-039** Manager realtime stream with domain statuses (08 §8.3)
- [ ] **SRV-040** Log partition creation and retention (08 §13)
- [ ] **SRV-041** Generated server SDKs: PHP, Python, Java, C#, Go (00 §6)

### Renderer and web SDK

- [ ] **REN-050** Validators in the selection engine: orphans, consecutive, minimum (04 §16)
- [ ] **REN-051** Group selection with one tap (04 §16)
- [ ] **REN-052** Filters: dim objects that do not match (04 §3)
- [ ] **REN-053** Section drill-down by tap in overview (04 §10)
- [ ] **REN-054** Multiple floors: switching, per-floor fit (04 §3)
- [ ] **REN-055** Areas, tables and booths: rendering, states, hit testing (04 §7, §15)
- [ ] **REN-056** Static and spotlight modes (05 §12)
- [ ] **REN-057** Manager mode: true statuses, click, box and row selection (05 §12)
- [ ] **REN-058** `selectBestAvailable` through the adapter (05 §6)
- [ ] **REN-059** `toImage` (05 §6)
- [ ] **WEB-030** Filter controls, floor selector, area quantity sheet (05 §7)
- [ ] **WEB-031** Vue and Svelte wrappers (05 §11)
- [ ] **WEB-032** Locale packs for six languages and right-to-left layout (05 §3.4)
- [ ] **WEB-033** `sessionToken` support with refresh function (05 §4)

### Console

- [ ] **CON-020** Event manager modes: rows, sections, statuses, channels, book, static (09 §4.2)
- [ ] **CON-021** Object and order search in the manager (09 §4.2)
- [ ] **CON-022** Channels tab (09 §4.3)
- [ ] **CON-023** Activity tab with filters and export (09 §4.3)
- [ ] **CON-024** Chart version upgrade action with conflict report (09 §4.3)
- [ ] **CON-025** Reports screens and order lookup (09 §5)
- [ ] **CON-026** Webhooks screens (09 §6)
- [ ] **CON-027** Custom statuses and validation settings (09 §7)
- [ ] **CON-028** Embedded event manager and `@cavea/manager-embed` (09 §4.2)

### Designer

- [ ] **DSN-050** Table tool and inspector (07 §5.4)
- [ ] **DSN-051** Booth tool and inspector (07 §5.4)
- [ ] **DSN-052** Area tool with editable vertices and capacity (07 §5.4)
- [ ] **DSN-053** Floors: switcher, add, reorder, dimmed reference (07 §5.6)
- [ ] **DSN-054** Icons palette and measure tool (07 §4)
- [ ] **DSN-055** Safe mode and publish impact report (07 §11)
- [ ] **DSN-056** Offer event upgrades after publish (07 §11)
- [ ] **DSN-057** Embedded designer: route, embed token, `postMessage` API, `@cavea/designer-embed` (07 §13)
- [ ] **DSN-058** White-label options: accent, logo, hidden items (07 §13)
- [ ] **DSN-059** CSV import and scale calibration (07 §12)
- [ ] **DSN-060** seats.io chart importer (07 §12, 14 §6)

### Testing

- [ ] **TST-040** Rule and best-available table tests, properties, WASM and native parity (13 §3.4)
- [ ] **TST-041** Load scenarios L3, L4, L5 (13 §7.2)
- [ ] **TST-042** Webhook delivery tests: ordering, retries, signature (08 §9)

---

## M6 · Accessibility, fallbacks, mobile wrappers → public v1

### Accessibility

- [ ] **A11Y-010** Full keyboard model: all keys in the table, overview navigation, focus memory (11 §3)
- [ ] **A11Y-011** Two-tone focus ring and camera follow (11 §3)
- [ ] **A11Y-012** Proxy name patterns for every object kind and state (11 §5)
- [ ] **A11Y-013** Polite and assertive announcements for every pattern listed (11 §5)
- [ ] **A11Y-014** List view fed by the engine (11 §4)
- [ ] **A11Y-015** List view fed by JSON for environments without WASM or canvas (11 §4)
- [ ] **A11Y-016** Non-colour cues for every state; optional category letter marks (11 §6)
- [ ] **A11Y-017** Colour-blind-safe palettes with a simulation test (11 §6)
- [ ] **A11Y-018** Forced-colours theme from system colours (11 §6)
- [ ] **A11Y-019** Reduced motion honoured everywhere (11 §6)
- [ ] **A11Y-020** Reflow of built-in controls at 200% text and 320 px (11 §6)
- [ ] **A11Y-021** Expiry warning dialog with "more time" and on-demand time left (11 §7)
- [ ] **A11Y-022** Automated checks: axe, proxy and announcement unit tests, contrast test (11 §9.1)
- [ ] **A11Y-023** Manual matrix: all eight journeys on every setup (11 §9.2, §9.3)
- [ ] **A11Y-024** Sessions with three screen-reader users and one keyboard-only user; fix blockers (11 §9.4)
- [ ] **A11Y-025** Console passes WCAG 2.2 AA checks (11 §8)
- [ ] **A11Y-026** Accessibility statement and integration guide (11 §10)

### Renderer

- [ ] **REN-070** Canvas 2D fallback backend driven by the draw list (04 §20.4)
- [ ] **REN-071** Capability ladder with forced-fallback option and reporting (04 §20.5)
- [ ] **REN-072** Dynamic resolution under sustained slow frames (04 §21)
- [ ] **REN-073** Fallback tests: WebGL2 disabled, WASM blocked (13 §6.1)

### Mobile, phase A

- [ ] **MOB-001** Shared host page and versioned asset bundling (06 §2.1)
- [ ] **MOB-002** Message bridge protocol: options, callbacks, methods (06 §2.1)
- [ ] **MOB-003** iOS SDK on `WKWebView` with the Swift API (06 §1, §2)
- [ ] **MOB-004** Android SDK on `WebView` with the Kotlin API (06 §1, §2)
- [ ] **MOB-005** WebView hardening and gesture, inset, theme and text-size pass-through (06 §2.2)
- [ ] **MOB-006** React Native component over the native views (06 §2.1)
- [ ] **MOB-007** Flutter plugin over the native views (06 §2.1)
- [ ] **MOB-008** Custom state adapter interface on each platform (06 §1)
- [ ] **MOB-009** Four example apps and UI tests (06 §5)
- [ ] **MOB-010** Publish: Swift package, Maven, npm, pub.dev (06 §3.2)

### Documentation and launch

- [ ] **DOC-020** Migration guide from seats.io with concept and API mapping (14 §6)
- [ ] **DOC-021** State import script from an incumbent's reports (14 §6)
- [ ] **DOC-022** Mobile guides for four platforms (14 §7)
- [ ] **DOC-023** Comparison page, honest about gaps (14 §2)
- [ ] **DOC-024** Public benchmark page with method and scripts (13 §6.2)
- [ ] **DOC-025** Performance and accessibility integration guides (05 §10, 11)
- [ ] **VAL-030** Reference customer agreed and case study written (14 §10)
- [ ] **VAL-031** Search landing pages (14 §5)
- [ ] **VAL-032** Open-source the wrappers, examples and benchmark harness (14 §5)
- [ ] **VAL-033** Public launch checklist complete; announce (14 §10)

---

## M7 · AI import

- [ ] **AI-001** Collect the 40-plan corpus and record sources (10 §7)
- [ ] **AI-002** Draw ground truth for the corpus in the designer (10 §7)
- [ ] **AI-003** Evaluation harness computing all five metrics (10 §7)
- [ ] **AI-004** Import service skeleton and job API (10 §6)
- [ ] **AI-005** PDF ingest with PDFium; vector or raster classification (10 §3 stage 0)
- [ ] **AI-006** Vector path extraction, shape signatures, seat cluster detection (10 §3 stage 1a)
- [ ] **AI-007** PDF text extraction with positions (10 §3 stage 1a)
- [ ] **AI-008** Row fitting: neighbour graph, chains, line and arc fits, merging (10 §3 stage 2)
- [ ] **AI-009** Block grouping into parametric row blocks (10 §3 stage 3)
- [ ] **AI-010** Section clustering and naming from nearby text (10 §3 stage 4)
- [ ] **AI-011** Label association and numbering-scheme fitting (10 §3 stage 5)
- [ ] **AI-012** Raster normalisation, component detection, size histogram (10 §3 stage 1b)
- [ ] **AI-013** Template-matching refinement and candidate scoring (10 §3 stage 1b)
- [ ] **AI-014** Model provider trait, structured-output schema, retry and "unknown" handling (10 §5)
- [ ] **AI-015** Label reading from indexed crops; section naming; stage location (10 §3 stages 5–6)
- [ ] **AI-016** Chart document assembly with aligned reference image and confidences (10 §3 stage 7)
- [ ] **AI-017** Review interface: overlay, open-question list, fix tools, capacity check, accept (10 §4)
- [ ] **AI-018** Run the evaluation; iterate until the ship gate is met (10 §7)
- [ ] **AI-019** Measure cost per import; test cheaper models against the corpus (10 §5, §8)
- [ ] **AI-020** Hosted service deployment, credits, file deletion (10 §6)
- [ ] **AI-021** Terms and data processing agreement for the hosted service (12 §8)

---

## M8 · Native mobile renderer

- [ ] **MOB-020** `wgpu` backend implementing `GpuBackend` (04 §20.3)
- [ ] **MOB-021** Backend parity golden tests between `glow` and `wgpu` (13 §6.1)
- [ ] **MOB-022** `cavea-mobile` UniFFI surface: options, callbacks, methods (06 §3.2)
- [ ] **MOB-023** iOS view: `CAMetalLayer`, display link, touches, lifecycle, render thread (06 §3.1)
- [ ] **MOB-024** iOS text rasteriser with CoreText and image decoding (06 §3.1)
- [ ] **MOB-025** Android view: `SurfaceView`, `Choreographer`, touches, surface lifecycle (06 §3.1)
- [ ] **MOB-026** Android text rasteriser and image decoding (06 §3.1)
- [ ] **MOB-027** Rust networking adapter with host override (06 §3.1)
- [ ] **MOB-028** Platform gesture constants and optional haptics (06 §3.4)
- [ ] **MOB-029** Coexistence with parent scroll views on both platforms (06 §3.1)
- [ ] **MOB-030** Accessibility elements on iOS and virtual nodes on Android (06 §3.5)
- [ ] **MOB-031** Native list-view screens (06 §3.5)
- [ ] **MOB-032** React Native Fabric component on the native renderer (06 §3.3)
- [ ] **MOB-033** Flutter `Texture` path with platform-view fallback (06 §3.3)
- [ ] **MOB-034** Automatic fallback to the WebView path (06 §3.7)
- [ ] **MOB-035** Size and performance budgets measured on reference phones (06 §3.6)
- [ ] **MOB-036** VoiceOver and TalkBack journeys; release (06 §5)

---

## M9 · Scale and enterprise

- [ ] **ENG-040** Active–passive failover by lease takeover, with simulation tests (08 §4.8)
- [ ] **ENG-041** Multi-node: ownership distribution and command forwarding (08 §4.8)
- [ ] **ENG-042** Node-to-node delta streams for remote subscribers (08 §4.8)
- [ ] **ENG-043** Seasons: season actor, atomic booking across events (02 §11)
- [ ] **SRV-050** Season endpoints (02 §11)
- [ ] **SRV-051** OIDC single sign-on for the console (09 §2)
- [ ] **SRV-052** Restricted API keys (12 §3 T4)
- [ ] **SRV-053** Private charts with signed URLs (03 §4.6)
- [ ] **SRV-054** S3-compatible asset storage (08 §13)
- [ ] **SRV-055** Cluster-wide rate limiting (08 §12)
- [ ] **WEB-040** Worker mode on `OffscreenCanvas` (04 §23)
- [ ] **REN-080** View-from-seat images in the tooltip (00 §6)
- [ ] **REN-081** Minimap (00 §6)
- [ ] **REN-082** Print mode (00 §6)
- [ ] **DSN-070** SVG and DXF import (07 §12)
- [ ] **DSN-071** SVG export (07 §12)
- [ ] **OPS-010** Helm chart (08 §14)
- [ ] **OPS-011** CDN guide for snapshots and chart files (08 §8.5)
- [ ] **TST-050** Chaos: kill the active node of a pair (13 §7.3)
- [ ] **TST-051** Soak scenario L6 (13 §7.2)
- [ ] **A11Y-030** External accessibility audit and conformance report (11 §1)
- [ ] **SEC-020** External security review of engine and authentication (12 §6)
