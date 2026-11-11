# 01 · Architecture

**Status:** Draft v0.1 · 2026-11-10

## 1. System overview

```
          AUTHORING                                  SELLING
┌──────────────────────────┐            ┌────────────────────────────────┐
│ Console SPA (SolidJS)    │            │ Customer checkout page or app  │
│  ├ Designer              │            │  └ @cavea/renderer             │
│  │   (edit core + render │            │      (Rust → WASM + WebGL2)    │
│  │    core, both WASM)   │            └───────┬──────────────┬─────────┘
│  └ Event manager         │       chart .ccf   │              │ snapshot + deltas
└────────────┬─────────────┘   (immutable URL,  │              │ (WebSocket / SSE)
             │ REST             CDN-cacheable)  │              │ hold / release
             │ (session cookie                  │              │ (HTTPS, publishable key)
             │  or embed token)                 ▼              ▼
┌────────────▼───────────────────────────────────────────────────────────┐
│ cavea-server — one Rust binary, hosted by the customer                 │
│   REST API · chart compiler · event actors (in-memory seat state,      │
│   one writer per event) · realtime hub · webhooks · console assets     │
└────────────┬───────────────────────────────────────────▲───────────────┘
             │ SQL                                        │ book / release /
             ▼                                            │ best-available
     ┌───────────────┐                         ┌──────────┴───────────┐
     │  PostgreSQL   │                         │ Customer backend     │
     └───────────────┘                         │ (orders, payments)   │
                                               └──────────────────────┘
```

Three flows matter:

1. **Authoring.** The designer edits a chart document (JSON). Publishing compiles it to an immutable binary chart (CCF) with a content hash in its URL.
2. **Buying.** The renderer fetches the CCF, the state snapshot and its own WASM in parallel, draws, then subscribes to deltas. A tap optimistically selects the seat and sends a hold. After payment the customer's backend books the held seats.
3. **Operating.** Console users block, release and book seats through the same server commands.

The renderer also runs with **no Cavea server at all**: the host passes seat state through an adapter (see `05-web-sdk.md` §4).

## 2. Components

| Component | Language | Runs where | Spec |
|---|---|---|---|
| `cavea-core` | Rust | Everywhere | Chart model, hit testing, selection rules, best-available, camera maths |
| `cavea-format` | Rust | Everywhere | CCF reader and writer, wire protocol codecs |
| `cavea-compiler` | Rust | Server, designer (WASM), CLI | Source document → CCF, validation |
| `cavea-render` | Rust | Web (WASM), mobile | Scene, level of detail, draw lists, GPU backends |
| `cavea-web` | Rust + TypeScript | Browser | DOM glue, input, accessibility layer, public API |
| `cavea-edit` | Rust | Browser (WASM) | Editor document, commands, undo, geometry tools |
| `cavea-engine` | Rust | Server | Event actor, state machine, storage trait |
| `cavea-server` | Rust | Customer infrastructure | HTTP, realtime, webhooks, auth, console hosting |
| `cavea-mobile` | Rust + Swift + Kotlin | iOS, Android | FFI surface, native views |
| `cavea` (CLI) | Rust | Developer machines, CI | Compile, validate, inspect, generate synthetic charts, benchmark |
| Console and designer UI | TypeScript, SolidJS | Browser | Chrome around the WASM canvases |

## 3. Repository layout

One monorepo: a cargo workspace and a bun workspace side by side.

```
cavea/
├─ Cargo.toml                 # workspace
├─ package.json               # bun workspaces
├─ crates/
│  ├─ cavea-core/             # no I/O, no async, minimal dependencies
│  ├─ cavea-format/           # CCF + wire protocol; fuzzed
│  ├─ cavea-compiler/
│  ├─ cavea-render/
│  │  ├─ src/backend/glow.rs  # WebGL2 / GLES3
│  │  ├─ src/backend/wgpu.rs  # native
│  │  └─ shaders/*.wgsl       # single source; GLSL generated in build.rs
│  ├─ cavea-web/              # wasm-bindgen entry point for the renderer
│  ├─ cavea-edit/             # wasm-bindgen entry point for the designer
│  ├─ cavea-engine/
│  ├─ cavea-server/
│  ├─ cavea-mobile/           # UniFFI definitions
│  ├─ cavea-cli/
│  └─ cavea-testkit/          # simulators, generators, golden-image helpers
├─ packages/
│  ├─ renderer/               # @cavea/renderer (TypeScript wrapper + WASM)
│  ├─ react/  vue/  svelte/   # thin wrappers
│  ├─ designer-embed/         # @cavea/designer-embed (iframe helper)
│  ├─ manager-embed/
│  └─ server-sdk/             # @cavea/server-sdk (Node)
├─ apps/
│  ├─ console/                # SolidJS SPA: console + designer + event manager
│  ├─ demo/                   # public demo and benchmark page
│  └─ docs/                   # documentation site
├─ mobile/
│  ├─ ios/  android/          # Swift package, Android library
│  └─ react-native/  flutter/
├─ sdks/                      # generated server SDKs (PHP, Python, Java, C#, Go)
├─ deploy/                    # Dockerfile, compose file, Helm chart
├─ bench/                     # chart corpus, traces, load-test scenarios
├─ xtask/                     # build orchestration (cargo xtask ...)
└─ docs/                      # this spec set
```

Dependency rule: `cavea-core` and `cavea-format` depend on nothing in the workspace except each other. `cavea-telemetry` (the server's logging and request spans) depends on nothing in the workspace. Nothing depends on `cavea-server`.

## 4. Technology choices

Each row is argued in `DECISIONS.md`.

| Layer | Choice | Main reason | Rejected |
|---|---|---|---|
| Shared logic | Rust (edition 2024) | One implementation of format, hit testing and selection rules for browser, server and mobile; no garbage-collection pauses | TypeScript core (cannot be shared with native mobile or a Rust server) |
| Web GPU API | WebGL2 through `glow` | Universal. WebGPU reached about 84% of page views in August 2026, which is not enough for a checkout | WebGPU only; `wgpu` on the web (one measurement: 561 KB brotli with its WebGL fallback, 37 KB without) |
| Native GPU API | `wgpu` | One backend covers Metal, Vulkan and GLES | Hand-written Metal plus GLES |
| Shaders | WGSL, translated to GLSL ES 3.00 at build time by `naga` | One source of truth and no translator shipped to the browser | Maintaining two shader sets by hand (the fallback if the build-time path fails its spike) |
| Web fallback | Canvas 2D renderer, then accessible list view | A buyer must always be able to buy | Failing with an error |
| Text | Baked MSDF atlas (digits, Latin) plus platform rasteriser | Sharp at every zoom for the common case; every script supported without shipping a shaping engine | Shipping a full text stack in WASM |
| Shape tessellation | `lyon`, at compile time | The browser receives ready triangles | Tessellating in the browser |
| WASM glue | `wasm-bindgen`, hand-written TypeScript surface | Small output and full control of the boundary | A JavaScript framework inside the renderer |
| Designer and console chrome | SolidJS, TypeScript, Vite, Tailwind, Kobalte | Fine-grained updates and a small runtime; the canvas is not its job | React (larger runtime); Rust UI frameworks (weaker component ecosystem) |
| Server runtime | tokio, axum, tower | Mature, fast, the default in the ecosystem | actix-web (no decisive advantage) |
| Database | PostgreSQL 15 or newer through `sqlx` | The only thing a customer must run besides our binary | Adding Redis, Kafka or NATS as requirements |
| State engine | Single-writer actor per event, group commit | Atomic check-and-claim in memory; best-available needs the whole picture anyway | Row-level SQL claims; Redis scripts |
| Realtime | Binary WebSocket, SSE fallback, cacheable HTTP snapshot | A CDN can absorb the read storm of an on-sale | Polling only |
| API description | OpenAPI 3.1 generated by `utoipa` | Server SDKs are generated, not written | Hand-written SDKs |
| Mobile bindings | UniFFI | Generates Swift and Kotlin from one definition | Hand-written JNI and C headers |
| Observability | `tracing`, OpenTelemetry, Prometheus endpoint | Standard tooling for self-hosters | Proprietary telemetry |
| Allocator (server) | mimalloc | Lower tail latency under many small allocations | System allocator |
| Build | cargo + bun, `cargo xtask`, `wasm-opt`, GitHub Actions with aggressive caching | Reproducible with size gates | Ad hoc scripts |
| Rust quality gates | `cargo fmt --check`, `cargo clippy` with warnings denied and a moderate lint set | Strict enough to catch real mistakes without pedantic noise | Clippy `pedantic` and `nursery` wholesale |
| TypeScript quality gates | bun as package manager and script runner; oxc for linting and formatting; `tsc` in strict mode for type checking | Fast tooling, strict types | ESLint and Prettier; npm or pnpm |

## 5. Performance budgets

All figures are **targets** until `13-testing-benchmarks.md` reports measurements. Budgets are enforced in CI where a machine can measure them, and per release on the reference devices otherwise.

**Reference devices** (choose and buy in M0): one low-to-mid Android phone about three years old in the US$200–250 class, one older iPhone still receiving updates, one current iPhone, one laptop with integrated graphics.

### Renderer

| Metric | Target |
|---|---|
| Renderer payload: JavaScript + WASM + font atlas, brotli | ≤ 150 KB; hard cap 200 KB (CI gate) |
| Chart-ready, 20,000 seats, desktop | ≤ 400 ms |
| Chart-ready, 20,000 seats, reference Android on 4G | ≤ 1.0 s |
| Chart-ready, 100,000 seats, desktop | ≤ 800 ms |
| Chart-ready, 100,000 seats, reference Android on 4G | ≤ 2.0 s |
| Chart-ready, 200,000 seats, desktop | ≤ 1.0 s (a competitor claims 1.95 s) |
| Frame time while panning, 95th percentile, desktop | ≤ 8.3 ms (120 Hz) |
| Frame time while panning, 95th percentile, reference Android, 100,000 seats | ≤ 16.6 ms |
| Tap to visible selection | Within one frame |
| Applying 10,000 seat status changes | ≤ 2 ms |
| Memory, 100,000 seats (WASM heap + GPU buffers) | ≤ 40 MB |
| Compiled chart size, 100,000 seats, brotli | ≤ 300 KB |
| Idle CPU | Zero: no frames are drawn when nothing changes |

Renderer payload measured locally, 2026-11-10 (REN-003), not a device result: the minimal renderer (WebGL2 context, `glow` backend, `shape` program, engine skeleton; no text, font atlas, gestures or chart decoding yet) is **37,298 B brotli**: 5,944 B of JavaScript glue and 31,354 B of WASM after `wasm-opt -Oz` (binaryen 132), as `cargo xtask build-web` reports. Without `wasm-opt` the WASM is 40,221 B. The `wasm-release` profile uses `opt-level = 3`, which beat `"s"` (32,846 B) and `"z"` (32,604 B) after `wasm-opt`. The default allocator is kept: `lol_alloc` saved 0.9 KB, which does not justify a dependency and an `unsafe` global allocator.

With the web SDK (WEB-001 to WEB-005) and `caveaServer` (WEB-020 to WEB-025), measured locally 2026-11-11: **109,658 B brotli**: 15,453 B for the minified bundle of `@cavea/renderer` (the SDK, the server adapter and the wasm-bindgen glue), 84,743 B of WASM and 9,462 B of glyph atlas (`13-testing-benchmarks.md` §6.3). With dynamic text, section labels and text objects (REN-026): **117,783 B** (15,911 B bundle, 92,410 B WASM, 9,462 B atlas), +8.1 KB; the 150 KB target leaves about 32.2 KB for row labels, icons, the built-in interface, the accessibility layer and the fallback ladder.

"Chart-ready" means the first frame in which the buyer can select a seat, measured from the start of renderer initialisation with a cold cache, excluding the host page.

### Server

| Metric | Target |
|---|---|
| Hold request, 99th percentile, at 2,000 holds per second (4 vCPU, PostgreSQL on the same network) | ≤ 15 ms |
| Commands per second per event, sustained | ≥ 20,000 (bounded by PostgreSQL commit rate) |
| Snapshot request, 99th percentile | ≤ 3 ms (served from memory) |
| Commit to delta delivered, 95th percentile | ≤ 250 ms |
| Concurrent realtime subscribers per node (4 vCPU, 8 GB) | ≥ 50,000 |
| Loading an event with 100,000 seats into memory | ≤ 300 ms |
| Best-available over 100,000 seats | ≤ 1 ms |

### Designer

| Metric | Target |
|---|---|
| Open a 100,000-seat chart | ≤ 2 s |
| Drag 5,000 selected seats | 60 fps |
| Undo or redo | ≤ 16 ms |
| Compile and preview 100,000 seats in the browser | ≤ 500 ms |

Edit core measured locally, 2026-11-11 (DSN-001..003), native release build on an AMD Ryzen 9 7900X, not in a browser: on `stadium-100k` (370 nodes, 399 KB of JSON), parse 3.3 ms and load into the editor 0.25 ms; a seat-pitch change on one block 0.005 ms; one drag step of 50 blocks (18,666 seats) 0.002 ms; undo or redo of either at most 0.002 ms; deleting 50 blocks 0.016 ms and undoing it 0.011 ms (`cargo test --release -p cavea-edit measure_ -- --ignored --nocapture`). Expansion, the spatial index and drawing are not included.

### Where the time goes

Drawing 100,000 seats is one instanced draw call, so the GPU is not the constraint. The budget is spent on:

1. Network: fetching the WASM, the chart and the snapshot. Run all three in parallel and keep each small.
2. Decode: turning the compiled chart into GPU buffers. Keep it a linear pass with no allocation per seat.
3. Text: seat numbers and section names. Build glyph buffers lazily and only for what is visible.
4. The JavaScript boundary: never cross it per seat.
5. DOM overlays: tooltips, legend, accessibility proxies. Keep them few and update them outside the frame loop.

## 6. Cross-cutting rules

### Versioning and compatibility

- **CCF** carries a format version. Readers must skip unknown chunks. A reader of version *n* reads every version ≤ *n*.
- **Wire protocol** carries a version in the first frame. Server and renderer negotiate the highest common version.
- **REST API** is versioned in the path (`/v1`). Additive changes only within a version.
- **Renderer and server** may be upgraded independently. The server supports renderers from the previous two minor versions.
- **Database migrations** are forward-only, embedded in the binary, and run at startup behind an advisory lock.

### Errors

- REST errors use `application/problem+json` with a stable machine code (`code`), a human message and a `details` object. Codes are listed in `08-server.md` §6.4.
- The renderer never throws into the host page after initialisation. It reports through `onError` and degrades (see `05-web-sdk.md` §9).
- Rust library crates return typed errors and do not panic on bad input. `cavea-format` treats every byte as hostile.

### Configuration

- Server: environment variables with a `CAVEA_` prefix, or a TOML file. Every setting has a safe default.
- Renderer: one options object, validated at start, with unknown keys reported in development builds.

### Logging and telemetry

- The server logs structured JSON. No personal data is logged: seat labels, hold tokens (hashed), order references (opt-in).
- The renderer collects timing marks and exposes them through `onMetrics`. It sends nothing anywhere by itself.
- No product phones home. Update checks and usage statistics are opt-in.

### Internationalisation

- All built-in strings live in message catalogues (ICU MessageFormat).
- Seat labels, section names and category names are data and are never translated by us.
- Layouts must work right-to-left.

### Build and release

- `cargo xtask build-web` produces the WASM, runs `wasm-opt`, generates TypeScript types and enforces the size gate against a committed baseline (`13-testing-benchmarks.md` §6.3).
- `cargo xtask release` builds the server binary and Docker image, signs both, and writes an SBOM.
- Every release publishes: Docker image, npm packages, Swift package, Android library, changelog, upgrade notes.

### Supply chain

- Lockfiles are committed and CI installs from them (`--locked`, `--frozen-lockfile`). `cargo-deny` and `cargo-audit` are not CI jobs; CI is kept to format, lint, type and build checks (`AGENTS.md`). npm packages are published with provenance.
- New dependencies in `cavea-core`, `cavea-format` and `cavea-web` need a size and necessity justification in the pull request.
