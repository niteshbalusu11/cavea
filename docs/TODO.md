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
- [ ] **REN-004** Spike: pan, pinch and wheel zoom with inertia (04 §14)
- [ ] **REN-005** Spike: hit test and tap-to-select with local state (04 §15)
- [ ] **REN-006** Spike: frame-time meter and timing marks on the page (04 §24)
- [ ] **TST-001** Choose and buy the reference devices (01 §5)
- [ ] **TST-002** Record spike frame times and load times on each reference device (13 §6.2)

---

## M1 · Core, format, web renderer

### Foundation

- [x] **FND-003** Scaffold crates `cavea-core`, `cavea-format`, `cavea-compiler`, `cavea-render`, `cavea-web`, `cavea-cli`, `cavea-testkit` with the dependency rule enforced (01 §2, §3)
- [x] **FND-004** `cargo xtask build-web`: build, `wasm-opt`, brotli, emit size report (01 §6)
- [ ] **FND-005** Payload size gate in CI with a hard cap and a +3% warning (13 §6.3)
- [x] **FND-006** Release profile tuning for WASM: LTO, `panic = "abort"`, optimisation level, allocator choice; measure each (01 §5)
- [ ] **FND-007** Generate TypeScript types from Rust for options, events and payloads (05 §3, §5)
- [x] **FND-008** Workspace lint configuration: moderate clippy set, strict `tsc`, oxc rules (01 §4)
- [ ] **FND-009** Changelog tooling and commit conventions (01 §6)
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
- [ ] **FMT-010** Sections: child transforms, automatic outline as concave hull (03 §2.3)
- [ ] **FMT-011** Row segmentation at gaps and removed seats (03 §3.1 step 5)
- [ ] **FMT-012** Label templating and uniqueness check (03 §3.1 steps 6–7)
- [ ] **FMT-013** Validation errors E001–E010 with node ids (03 §3.2)
- [ ] **FMT-014** Validation warnings W101–W110 (03 §3.2)
- [ ] **FMT-015** Seat rank computation, versioned (03 §3.3)
- [ ] **FMT-016** `idx` ordering and chunking to at most 4,096 seats (03 §3.1 steps 9–10)
- [ ] **FMT-017** Section label anchor by pole of inaccessibility and maximum font size (03 §3.1 step 11)
- [ ] **FMT-018** Tessellate shapes, outlines and table shapes with `lyon` (03 §3.1 step 12)
- [ ] **FMT-019** Decoration nodes: shape, text, icon, image (03 §2.7)
- [x] **FMT-020** CCF writer: header, directory, all chunks and encodings (03 §4.1, §4.2)
- [x] **FMT-021** CCF reader: validated, single pass, no panics (03 §4.4)
- [ ] **FMT-022** Emit public and full artefacts with content hash (03 §3.1 steps 14–15)
- [ ] **FMT-023** Schema migration framework with stored samples (03 §2.8)
- [ ] **FMT-024** CLI: `cavea compile`, `validate`, `inspect`, `gen` (01 §2)
- [ ] **FMT-025** All synthetic presets, seeded (03 §7)
- [ ] **FMT-026** Measure compiled sizes against targets; record (03 §4.5)

### Renderer engine

- [x] **REN-010** Engine command and event types (04 §3)
- [ ] **REN-011** Host service traits: clock, frame scheduler, text rasteriser, image source (04 §2)
- [ ] **REN-012** Scene arrays loaded from CCF; per-row, segment, section, chunk tables (04 §5)
- [x] **REN-013** `GpuBackend` trait and fixed pipeline enum (04 §20.1)
- [x] **REN-014** `glow` backend for WebGL2 with the specified context attributes (04 §20.2)
- [x] **REN-015** Shader build pipeline: WGSL sources, generated GLSL, or the hand-written fallback (DECISIONS D-05)
- [x] **REN-016** `shape` program: instanced quads, signed-distance shapes, stroke, anti-aliasing (04 §8)
- [x] **REN-017** Seat static and dynamic buffers with the specified layout (04 §6)
- [ ] **REN-018** Palette texture and visual state mapping (04 §9)
- [ ] **REN-019** `mesh` program: decoration layers and section fills (04 §7)
- [ ] **REN-020** `image` program and lazy background image loading (04 §7)
- [ ] **REN-021** Level-of-detail levels with cross-fade and hysteresis (04 §10)
- [ ] **REN-022** Section availability counters and overview tint (04 §5, §10)
- [ ] **REN-023** Chunk culling and merging of visible ranges; stay under 64 seat draw calls (04 §11)
- [ ] **REN-024** MSDF atlas bake tool and `glyph` program (04 §12)
- [ ] **REN-025** Seat labels from row rules, built per chunk with an LRU of 64 (04 §12)
- [ ] **REN-026** Dynamic text atlas, size buckets, `text` program, browser rasteriser (04 §12)
- [ ] **REN-027** Row labels, section labels, text objects with greedy collision (04 §12)
- [ ] **REN-028** Icon atlas and seat icons: accessible, tick, cross (04 §7)
- [ ] **REN-029** Camera: fit, limits, clamping, `FlyTo` in log-zoom space (04 §13)
- [ ] **REN-030** Gesture recogniser: drag, pinch, wheel, trackpad, double tap, two-finger tap, long press, inertia (04 §14)
- [ ] **REN-031** Gesture handling modes and `touch-action` (04 §14)
- [ ] **REN-032** Hierarchical hit testing with tolerance (04 §15)
- [ ] **REN-033** Zoom-to-select on touch (04 §15)
- [ ] **REN-034** Selection engine: optimistic select, hold request and result, revert (04 §16)
- [ ] **REN-035** Maximum and minimum selection rules (02 §12)
- [ ] **REN-036** Snapshot application in one pass (04 §17)
- [ ] **REN-037** Delta application with merged dirty ranges (04 §17)
- [ ] **REN-038** On-demand frame scheduling with dirty flags and an animation list (04 §18)
- [ ] **REN-039** Resize and device-pixel-ratio handling with caps (04 §21)
- [ ] **REN-040** Context loss and restoration (04 §20.2)
- [ ] **REN-041** Theme tokens to palette; light, dark and high-contrast themes (04 §19)
- [ ] **REN-042** Overlay pass: hover ring and focus ring (04 §7)
- [ ] **REN-043** Pulse on change, and reduced-motion handling (04 §17, §13)
- [ ] **REN-044** Timing marks and frame statistics (04 §24)
- [ ] **REN-045** Headless render to a pixel buffer for golden tests (04 §26)

### Web SDK

- [ ] **WEB-001** Package scaffold; loader that starts WASM, chart, state and atlas fetches together (04 §4, 05 §1)
- [ ] **WEB-002** `createSeatMap` with options validation and development warnings (05 §3)
- [ ] **WEB-003** `StateAdapter` interface, `staticState`, `customState` (05 §4)
- [ ] **WEB-004** Callbacks and the `SeatObject` payload (05 §5)
- [ ] **WEB-005** Methods: selection, pricing, theme, zoom, find, list, destroy (05 §6)
- [ ] **WEB-006** Shadow-root interface: zoom controls, legend, loading and error states (05 §7)
- [ ] **WEB-007** Tooltip with price, ticket-type choice and flags; host replacement (05 §7)
- [ ] **WEB-008** Toasts for refusals and rule violations (05 §7)
- [ ] **WEB-009** Pricing display and `formatPrice` (05 §3.1)
- [ ] **WEB-010** Theme tokens as CSS custom properties, applied live (05 §8)
- [ ] **WEB-011** Message catalogue in English, `messages` override, `locale` (05 §3.4)
- [ ] **WEB-012** `preloadSeatMap` and preload documentation (05 §10)
- [ ] **WEB-013** Script-tag build (05 §1)
- [ ] **WEB-014** React wrapper with one-to-one props and imperative ref (05 §11)
- [ ] **WEB-015** Server-side-rendering safety test with Next.js (05 §10)
- [ ] **WEB-016** Content Security Policy test page and documentation (05 §10)
- [ ] **WEB-017** Error model and degradation behaviours (05 §9)
- [ ] **WEB-018** Bundler compatibility checks: Vite, webpack 5, Rollup, esbuild (13 §8)

### Accessibility baseline

- [ ] **A11Y-001** Map as one tab stop with arrow-key virtual focus and Enter to select (11 §3)
- [ ] **A11Y-002** Hidden proxy element with rebuilt accessible name; polite live region for selections (11 §5)

### Testing

- [ ] **TST-010** Golden-image harness with tolerance and stored baselines (13 §6.1)
- [ ] **TST-011** Micro-benchmarks: decode, hit test, delta apply, label build (13 §6.3)
- [ ] **TST-012** Benchmark page with the scripted gesture trace and result collector (13 §6.2)
- [ ] **TST-013** Fuzz targets `ccf_decode` and `document_parse` with seed corpora (13 §5)
- [ ] **TST-014** Compiler round-trip and determinism tests across platforms (13 §4)
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
- [ ] **ENG-006** Commands: book with token, book direct, change status (08 §7.2)
- [ ] **ENG-007** All-or-nothing validation with per-object failure reasons (02 §4.3)
- [ ] **ENG-008** Hold expiry heap and internal expire command (08 §7.3)
- [ ] **ENG-009** Idempotency: in-memory window plus stored records (08 §6.1)
- [ ] **ENG-010** Public snapshot re-encoding and lock-free publication (08 §4.5)
- [ ] **ENG-011** Delta coalescing and the 60-second ring buffer (08 §8.3)
- [ ] **ENG-012** Ownership: acquire, epoch, lease renewal, fencing in commit (08 §4.7, §4.8)
- [ ] **ENG-013** Load from storage, including overdue expiries; idle eviction (08 §4.7)
- [ ] **ENG-014** Poison and reload on commit failure or panic (08 §4.6)
- [ ] **ENG-015** Mailbox bounds and overload replies (08 §4.3)
- [ ] **ENG-016** Actor registry with concurrent load de-duplication (08 §3)

### Server

- [x] **SRV-001** Server scaffold: axum, configuration, tracing, request ids (08 §2, §3, §13)
- [x] **SRV-025** `cavea-telemetry` crate: structured logging setup, HTTP request spans with request id, child spans for mailbox wait, apply and commit (AGENTS.md Logging, 08 §15)
- [ ] **SRV-002** Schema and embedded migrations under an advisory lock (08 §5)
- [ ] **SRV-003** PostgreSQL store: pipelined batch commit transaction (08 §4.4)
- [ ] **SRV-004** API keys: generation, hashing, prefixes, extractors (08 §11)
- [ ] **SRV-005** Workspaces and admin-key bootstrap (08 §11)
- [x] **SRV-006** Problem+json errors with the stable code table (08 §6.4)
- [ ] **SRV-007** Chart endpoints: create, draft with etag, validate, publish, versions (08 §6.2)
- [ ] **SRV-008** Compile on publish; store public, precompressed and full artefacts (03 §4.6)
- [ ] **SRV-009** Serve compiled charts with immutable caching and content negotiation (03 §4.6)
- [ ] **SRV-010** Asset upload and serving (08 §6.2)
- [ ] **SRV-011** Event endpoints: create, get, patch, archive (08 §6.2)
- [ ] **SRV-012** Hold-token endpoints (08 §6.2)
- [ ] **SRV-013** Seat operation endpoints: hold, release, book, change status (08 §6.2)
- [ ] **SRV-014** Objects query endpoint (08 §6.2)
- [ ] **SRV-015** Public endpoints with CORS and origin allow-list (08 §6.2, §11)
- [ ] **SRV-016** Session token verification and `require_session` (08 §11)
- [ ] **SRV-017** Idempotency-Key handling and mismatch detection (08 §6.1)
- [ ] **SRV-018** Rate limiting per key and IP with trusted-proxy handling (08 §12)
- [ ] **SRV-019** OpenAPI document generated and served (08 §6.1)
- [ ] **SRV-020** Health, readiness and Prometheus metrics (08 §15)
- [ ] **SRV-021** Graceful shutdown: flush, close subscribers, release leases (08 §14)
- [ ] **SRV-022** Startup load of events with outstanding holds (08 §4.7)
- [ ] **SRV-023** Dockerfile (distroless, multi-arch) and compose file (08 §14)
- [ ] **SRV-024** Body size, object count and JSON depth limits (12 §3 T19)

### Realtime

- [ ] **RT-001** Frame codecs in `cavea-format`, shared by server and clients (08 §8.1, §8.2)
- [ ] **RT-002** Snapshot encodings: raw, run-length, bitset; pick the smallest (08 §8.1)
- [ ] **RT-003** WebSocket endpoint: subscribe, hello, resume from `last_seq` (08 §8.3)
- [ ] **RT-004** Fan-out of shared pre-encoded frames (08 §4.5)
- [ ] **RT-005** Slow-consumer detection and RESYNC (08 §8.3)
- [ ] **RT-006** Ping, pong and idle timeouts (08 §8.3)
- [ ] **RT-007** SSE transport (08 §8.4)
- [ ] **RT-008** Polling with `after_seq` (08 §8.4)
- [ ] **RT-009** HTTP snapshot with ETag and cache headers (08 §8.5)
- [ ] **RT-010** PRIVATE frames for the caller's holds (08 §8.1)

### Web SDK

- [ ] **WEB-020** `caveaServer` adapter: binary snapshot, transport auto-selection, resync (05 §4)
- [ ] **WEB-021** Hold-on-select: ordered single-flight requests with coalescing (04 §16)
- [ ] **WEB-022** Session modes: start, continue, manual, none (05 §4)
- [ ] **WEB-023** Hold token persistence and restore after reload (05 §4)
- [ ] **WEB-024** Expiry callbacks and the optional hold timer (05 §5, §7)
- [ ] **WEB-025** Pause when hidden and resync on return (05 §4)
- [ ] **WEB-026** `@cavea/server-sdk` for Node, generated from OpenAPI with an ergonomic wrapper (05 §1)

### Testing

- [ ] **TST-020** Reference model and model-based property tests for the engine (13 §3.1)
- [ ] **TST-021** Deterministic simulation harness with the fault table (13 §3.2)
- [ ] **TST-022** Reconciler: `cavea verify` (13 §3.3)
- [ ] **TST-023** Load generator `cavea-loadgen` (13 §7.1)
- [ ] **TST-024** Scenarios L1 and L2 on reference hardware; record results (13 §7.2)
- [ ] **TST-025** Chaos: `kill -9` and PostgreSQL restart during load (13 §7.3)
- [ ] **TST-026** Authorisation matrix and tenant isolation tests (13 §10)
- [ ] **TST-027** Fuzz targets `wire_decode`, `snapshot_apply` and HTTP (13 §5)

---

## M3 · Designer MVP

### Edit core

- [ ] **DSN-001** `cavea-edit` crate: document arena and node types (07 §2)
- [ ] **DSN-002** Command framework with computed inverses (07 §2)
- [ ] **DSN-003** Undo and redo stack with coalescing for drags and scrubbing (07 §9)
- [ ] **DSN-004** Live scene: incremental expansion per changed node (04 §25)
- [ ] **DSN-005** Renderer extensions: chunk transforms and the overlay batcher (04 §25)
- [ ] **DSN-006** View models and their generated TypeScript types; throttled to one per frame (07 §2, §14)
- [ ] **DSN-007** Spatial index for snapping and selection (07 §5.2)

### Shell

- [ ] **DSN-008** Application shell: top bar, tool rail, canvas, inspector, panels, status bar; resizable and persistent (07 §3)
- [ ] **DSN-009** Command palette and shortcut help (07 §8)
- [ ] **DSN-010** All keyboard shortcuts, with macOS variants (07 §8)

### Tools

- [ ] **DSN-011** Select tool: click, shift-click, box, enter and exit nested levels (07 §5.1)
- [ ] **DSN-012** Move, nudge, rotate with pivot, resize semantics for blocks and shapes (07 §5.1)
- [ ] **DSN-013** Hand tool, zoom shortcuts, zoom to selection (07 §4)
- [ ] **DSN-014** Row block tool with live counts and numeric entry (07 §4)
- [ ] **DSN-015** Arc block tool (07 §4)
- [ ] **DSN-016** Single row tool with optional bend (07 §4)
- [ ] **DSN-017** Section tool and automatic outline; convert to polygon (07 §5.5)
- [ ] **DSN-018** Shape tools: rectangle, ellipse, polygon, line (07 §4)
- [ ] **DSN-019** Text tool (07 §4)
- [ ] **DSN-020** Image tool: upload, reference or background role, opacity, lock (07 §4)
- [ ] **DSN-021** Focal point tool (07 §4)
- [ ] **DSN-022** Category brush (07 §4)

### Panels and editing

- [ ] **DSN-023** Inspector for row blocks with the curve handle (07 §5.3)
- [ ] **DSN-024** Inspector for sections, shapes, text, images and chart settings (07 §5.5)
- [ ] **DSN-025** Row-level editing: add or remove seats, shift, insert aisle, override label, delete row (07 §5.3)
- [ ] **DSN-026** Seat-edit mode: remove, restore, flags, category, label, nudge (07 §5.3)
- [ ] **DSN-027** Numbering panel for rows and seats with canvas preview (07 §6)
- [ ] **DSN-028** Categories panel: add, rename, recolour, merge, delete with reassignment; contrast warning (07 §7)
- [ ] **DSN-029** Layers panel: virtualised tree, drag to reparent, lock, hide (07 §3)
- [ ] **DSN-030** Snapping: grid, edges and centres, equal spacing, row alignment, angles (07 §5.2)
- [ ] **DSN-031** Rulers and user guides (07 §5.2)
- [ ] **DSN-032** Align, distribute, flip, duplicate, array duplicate (07 §5.1)
- [ ] **DSN-033** Copy and paste, including between charts (07 §5.1)
- [ ] **DSN-034** Group, ungroup, order for decoration nodes (07 §5.1)

### Saving, validation, publishing

- [ ] **DSN-035** Autosave with etag and save-state indicator (07 §9)
- [ ] **DSN-036** IndexedDB safety net and recovery prompt (07 §9)
- [ ] **DSN-037** Single-editor lock with take-over (07 §9)
- [ ] **DSN-038** Continuous validation, panel, canvas markers (07 §10)
- [ ] **DSN-039** Preview with the buyer renderer, device widths and sample states (07 §11)
- [ ] **DSN-040** Publish flow: compile, diff, thumbnail upload (07 §11)
- [ ] **DSN-041** Export and import of chart document JSON (07 §12)
- [ ] **DSN-042** New-chart screen, three templates, first-run tour (07 §16)
- [ ] **DSN-043** "Insert row block" form as the non-pointer path (07 §15)
- [ ] **DSN-044** Performance pass on the 100,000-seat chart (07 §14)

### Testing

- [ ] **TST-030** Command inverse property tests (13 §9)
- [ ] **TST-031** End-to-end: draw, number, publish, create event, hold a seat (13 §9)
- [ ] **TST-032** Usability test: three new users draw the reference theatre; record times (ROADMAP M3)

---

## M4 · Console, licensing, docs → private beta

### Console

- [ ] **CON-001** Application shell, router, navigation, workspace switcher (09 §1)
- [ ] **CON-002** First-run setup with a one-time link (09 §2)
- [ ] **CON-003** Sign-in, sessions, CSRF, optional TOTP (09 §2, §10)
- [ ] **CON-004** Roles and permission checks on every route (09 §2)
- [ ] **CON-005** Charts list with thumbnails, search, card menu, empty state (09 §3.1)
- [ ] **CON-006** Chart detail: versions, events, settings (09 §3.2)
- [ ] **CON-007** Events list and create form (09 §4.1)
- [ ] **CON-008** Event manager, basic: live map, inspect, select, block, release, book (09 §4.2)
- [ ] **CON-009** Event settings tab (09 §4.3)
- [ ] **CON-010** API keys screen (09 §6)
- [ ] **CON-011** Quick-start snippets pre-filled with workspace values (09 §6)
- [ ] **CON-012** Workspace and members settings; invitations (09 §7)
- [ ] **CON-013** Audit log writes for sensitive actions (09 §4.2)
- [ ] **CON-014** Interface standards: tables, forms, feedback, themes, keyboard (09 §8)
- [ ] **CON-015** Embed the built console in the server binary with hashed assets (09 §9)

### Licensing

- [ ] **LIC-001** Licence file format and private signing tool (12 §7.2)
- [ ] **LIC-002** Server verification and behaviour by state; seat operations never blocked (12 §7.3)
- [ ] **LIC-003** Renderer watermark logic, checked after the first interactive frame (12 §7.4)
- [ ] **LIC-004** Console licence screen and banners (09 §7)
- [ ] **LIC-005** Test: expired and missing licences leave hold and book working (DECISIONS D-20)

### Security

- [ ] **SEC-001** Review the threat table against the implementation; file gaps (12 §3)
- [ ] **SEC-002** SSRF guard for outbound requests (12 §3 T8)
- [ ] **SEC-003** Login rate limiting and session rotation (12 §3 T9)
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
