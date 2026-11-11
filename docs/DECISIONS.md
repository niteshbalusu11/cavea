# Decisions

**Status:** Draft v0.1 · 2026-11-10

Each entry records what was decided, what was rejected and what would make us reopen it. Add a new entry rather than editing an old one; mark superseded entries.

---

## D-01 · Self-hosted, flat-price licence first

- **Decision:** Ship software the customer runs. No managed cloud until there are paying customers.
- **Why:** It is the open slot in the market, and it removes on-call duty from a one-person company.
- **Rejected:** A hosted per-seat service (competes head-on with three vendors and puts us on the critical path of every sale).
- **Cost:** Customers must run PostgreSQL and a container. Piracy is possible.
- **Revisit if:** Prospects repeatedly say they would buy only a hosted version.

## D-02 · One Rust core shared by browser, server and mobile

- **Decision:** Format decoding, hit testing, selection rules and best-available live in `cavea-core` and `cavea-format`, compiled to WASM, native server and mobile libraries.
- **Why:** The client and the server must agree exactly on orphan rules and seat adjacency. One implementation removes a class of bugs. It also gives predictable performance without garbage-collection pauses.
- **Rejected:** A TypeScript core (cannot be reused natively); separate implementations per platform.
- **Cost:** A WASM boundary to design carefully, and slower iteration on the web.

## D-03 · Web rendering on WebGL2 through `glow`, not `wgpu`

- **Decision:** The web backend calls WebGL2 directly from Rust through `glow`.
- **Why:** WebGL2 is available almost everywhere; WebGPU covered about 84% of page views in August 2026. For this workload (instanced quads) WebGPU brings no speed advantage. One published measurement put `wgpu` with its WebGL fallback at 561 KB brotli against 37 KB without it, and payload size is our main startup cost.
- **Rejected:** `wgpu` on the web; WebGPU only; Canvas 2D as the primary renderer; a general-purpose 2D library.
- **Cost:** Two GPU backends to keep in visual parity (see D-04, D-05).
- **Revisit if:** A measured `wgpu` web build with WebGL2 fits inside the payload budget.

## D-04 · Native mobile rendering through `wgpu`; WebView wrappers ship first

- **Decision:** Phase A wraps the web renderer in a WebView on each mobile platform, matching what every competitor offers. Phase B adds a native renderer using `wgpu`.
- **Why:** Customers need a mobile answer on day one. A native renderer is a genuine differentiator but is not needed to win the first customers.
- **Rejected:** Native first (delays everything); GLES on iOS (deprecated by Apple).
- **Revisit if:** Early customers do not care about native mobile; then Phase B drops in priority.

## D-05 · One WGSL shader source; GLSL generated at build time

- **Decision:** Shaders are written once in WGSL. `build.rs` uses `naga` to emit GLSL ES 3.00 for the `glow` backend. No shader translator ships to the browser.
- **Why:** Single source of truth without the size cost.
- **Risk:** Not yet proven for our shaders. Spike task `REN-002` confirms it.
- **Fallback:** Maintain the five GLSL shaders by hand, with golden-image tests guarding parity.

## D-06 · A compiled binary chart format separate from the JSON source

- **Decision:** The designer edits a JSON chart document. Publishing compiles it to CCF, a chunked binary file that the renderer decodes in a single linear pass.
- **Why:** Buyers' phones should not parse megabytes of JSON, tessellate shapes or compute labels. Compilation moves that work to publish time.
- **Rejected:** Shipping JSON; a general serialisation format (larger, and we want delta and run-length encodings tuned to rows of seats).
- **Cost:** A format to version and fuzz.

## D-07 · Parametric source model

- **Decision:** The chart document stores row blocks as parameters (rows, seats per row, pitch, curve, numbering scheme), not as individual seats. Seats are generated at compile time and in the editor's live scene.
- **Why:** It makes editing fast ("change the spacing of this block"), keeps documents small and diff-friendly, and is how venues think.
- **Cost:** Per-seat overrides need a stable identity (D-08).

## D-08 · Three identifiers per object: `oid`, `idx`, `label`

- **Decision:** A stable numeric `oid` for storage, a dense per-version `idx` for arrays and wire deltas, and a human `label` for the API.
- **Why:** Each job has a different best representation. Labels alone break when someone renames a row; indices alone break when a chart changes.
- **Rejected:** Labels as the only identifier (the leader's approach).

## D-09 · Single-writer event actor with PostgreSQL group commit

- **Decision:** Each live event's seat state is held in memory by one actor that processes commands sequentially, batches them, and commits each batch to PostgreSQL in one transaction before acknowledging.
- **Why:** Check-and-claim becomes a memory operation taking microseconds. Best-available and orphan rules need the whole event's state at once, which is awkward and slow in SQL. Sequence numbers and deltas become trivially ordered.
- **Rejected:** Row-level `INSERT … ON CONFLICT` claims (simple, but best-available needs a cached copy and retries, and a per-event sequence needs a lock anyway); Redis Lua scripts (a second required system, weaker durability).
- **Cost:** Ownership and fencing for failover; more testing effort. Mitigated by simulation testing.
- **Revisit if:** The actor's correctness cost proves higher than expected during M2. The fallback is the SQL-claims design behind the same `SeatStore` trait.

## D-10 · PostgreSQL is the only required dependency

- **Decision:** No Redis, message broker or object store is required. Compiled charts and small assets are stored in PostgreSQL; S3-compatible storage is optional.
- **Why:** Every extra system is a reason for a prospect's operations team to say no.

## D-11 · Events pin an immutable chart version

- **Decision:** Publishing creates a new version and never mutates an old one. Events move to a new version only by an explicit, validated upgrade.
- **Why:** State arrays are indexed by `idx`; immutability makes that safe and makes chart files cacheable forever.
- **Rejected:** Publish-in-place (the leader's approach, which their docs call irreversible).

## D-12 · The public sees only "available" or "unavailable"

- **Decision:** Public snapshots and deltas do not distinguish held from booked or blocked.
- **Why:** It leaks less commercial information and gives scalpers less signal.
- **Cost:** Manager views need a separate authenticated stream.

## D-13 · Commands over HTTP, state over WebSocket or SSE

- **Decision:** Holds and releases are plain HTTPS requests. Realtime connections only carry state downstream.
- **Why:** HTTP gives retries, idempotency keys, status codes and standard tooling. The realtime channel stays simple and can be replaced by SSE or polling where WebSockets are blocked.

## D-14 · Hold on select, with optimistic UI

- **Decision:** By default a tap marks the seat selected immediately and sends a hold; a failure reverts it with a message.
- **Why:** It is what buyers expect and what the incumbents do. The optimistic step keeps the tap response within one frame.
- **Alternative supported:** `session: "manual"`, where the host holds at checkout.

## D-15 · SolidJS for interface chrome

- **Decision:** The console and the designer's panels use SolidJS. All canvas drawing goes through the Rust renderer.
- **Why:** Small runtime and fine-grained updates. The chrome is not performance-critical, but the designer's inspector updates on every drag.
- **Rejected:** React (larger; acceptable, and the fallback if component availability hurts); a Rust UI framework.

## D-16 · Accessibility is a first-class feature

- **Decision:** The renderer ships a keyboard model, a live DOM proxy for the focused seat, and a full accessible list view as an alternative to the map.
- **Why:** It is a real sales argument: no competitor documents it, and customers selling in the EU have obligations for their checkout.
- **Cost:** A parallel interaction model to design and test with real screen readers.

## D-17 · No 3D

- **Decision:** No 3D venue or view-from-seat rendering.
- **Why:** Large effort, weak link to our wedge. View-from-seat photos (an image per section or seat) cover most of the value at P2.

## D-18 · Text: baked MSDF atlas plus platform rasteriser

- **Decision:** Digits and Latin letters come from a signed-distance-field atlas baked at build time. Any other text is rasterised by the platform (browser canvas, CoreText, Android canvas) into a dynamic atlas.
- **Why:** Seat numbers are the bulk of the text and must be sharp at every zoom at no runtime cost. Other scripts must work without shipping a shaping engine.

## D-19 · AI import is optional and always reviewed by a person

- **Decision:** PDF or image import produces a draft that the user corrects in the designer. It runs as an optional service; the core product works offline.
- **Why:** Detection will not be perfect. The promise is "most of the drawing done in a minute", not "no drawing".

## D-20 · Licence enforcement never blocks a sale

- **Decision:** An expired or missing licence shows a watermark and makes the console read-only. Holding and booking continue.
- **Why:** Breaking a customer's checkout over a billing lapse would be fatal to trust.

## D-21 · Main-thread rendering first, behind a command and event protocol

- **Decision:** The engine is driven only by serialisable commands and emits serialisable events. v1 runs on the main thread; a worker with `OffscreenCanvas` comes later without changing the engine.
- **Why:** A worker protects frame rate from a busy host page, but it complicates text rasterisation and the accessibility layer. The protocol keeps the option open at no cost.

## D-22 · REN-002 result: `naga` produces working GLSL ES 3.00 for the seat shader (2026-11-10)

- **Result:** D-05 holds for the `shape` program. `crates/cavea-render/build.rs` parses and validates `shaders/shape.wgsl` with `naga` 30.0.1 (features `wgsl-in` and `glsl-out` only, a build dependency) and writes GLSL ES 3.00 with `Version::Embedded { version: 300, is_webgl: true }` and no writer flags. The output compiled and linked in Chromium 157 (WebGL 2.0, ANGLE on SwiftShader, software rendering) and drew 100,000 seats with `gl.getError() == 0`. No hand-written fallback was needed.
- **What the risky features became:** `@builtin(vertex_index)` → `uint(gl_VertexID)`; `textureLoad` on a `texture_2d<f32>` → `texelFetch` on a `highp sampler2D`; `@interpolate(flat)` → `flat` varyings, including `uvec2`; `u32` and `vec2<u32>` vertex inputs → `layout(location = N) in uint`/`uvec2`, fed with `vertexAttribIPointer`; the uniform struct → a `std140` block.
- **What the backend must do because of GLSL ES 3.00:** it has no `layout(binding)`, so uniform blocks and samplers are bound by name. `naga` names each stage's block separately (`Globals_block_0Vertex`, `Globals_block_0Fragment`) and the sampler `_group_0_binding_1_vs`. `build.rs` reads these names from `naga`'s reflection output and emits them as constants, so nothing hard-codes them. Varyings are matched by `naga`'s generated names, which agree between stages.
- **Not yet proven:** the `mesh`, `glyph`, `text` and `image` programs, and a hardware GPU or another browser engine. (A Firefox headless screenshot was attempted; it is taken at the `load` event, before the WASM has started, so it proved nothing.) Revisit if one of those fails to translate.


## D-23 · The batch commit is one statement of data-modifying CTEs, not a pipelined transaction (2026-11-11)

- **Decision:** `PgStore` writes a batch (08 §4.4) as one SQL statement: the fence is an `UPDATE … RETURNING` CTE, every other part is a CTE fed by `UNNEST` arrays and gated on the fence, and the statement returns whether the fence matched. It runs outside an explicit transaction, so it is one round trip and atomic on its own.
- **Why:** 08 §4.4 asked for the eight statements in one round trip as a pipelined transaction. `sqlx` 0.9 does not pipeline: each `execute` waits for its reply, so a transaction of separate statements costs `BEGIN`, one round trip per non-empty part and `COMMIT` (5 for a one-command hold, up to 10 for a full batch). Measured locally (AMD Ryzen 9 7900X, PostgreSQL 18 in Docker on loopback, `synchronous_commit = on`, 0.08 ms round trip), both approaches commit in the same time because the WAL flush dominates: median 1.6 ms for 1 command, 3.8 ms for 100, 29 to 43 ms for 2,000. Across a real network each saved round trip is 0.2 to 1 ms per commit, which counts against the 15 ms hold budget.
- **Rejected:** a multi-statement transaction (more round trips, same atomicity); the simple-query protocol, which does pipeline several statements but takes no bind parameters, so values would have to be spliced into SQL text; a second PostgreSQL driver with pipelining (`tokio-postgres`), which duplicates `sqlx` in the build.
- **Cost:** one long statement instead of eight short ones, and a gate (`WHERE EXISTS (SELECT FROM fence)`) on every part. A refused commit costs a second query to say why.
- **Revisit if:** `sqlx` gains pipelining, or a part needs to read another part's writes (CTEs in one statement share a snapshot and cannot).

## D-24 · The server starts without an admin key; it never has a default one (2026-11-11)

- **Decision:** `CAVEA_ADMIN_KEY` is optional. When it is set it must be 32 to 512 printable ASCII characters, or the server refuses to start; when it is unset the server starts, logs a warning, and the admin endpoints (`/v1/workspaces`) refuse every request (`401`, or `403` for a valid API key, as on any admin route). Each new workspace is created with its first secret key, returned once in the creation response (08 §11).
- **Why:** 12 §4 read "required to be set explicitly". What it guards against is a default key. Requiring one at every start would make operators keep a key that can create tenants configured long after the last workspace exists; unsetting it is the safer steady state. A short key is refused because the admin key is not rate limited.
- **Revisit if:** console users and roles (09) give the operator another way to create workspaces, or an endpoint other than workspace creation needs the admin key.

## D-25 · Decoration carries its floor; strokes are built without `lyon` (2026-11-11)

- **Decision:** `TEXT`, `ICON` and `IMGS` records carry a floor, and `MESH` draw ranges are one per floor and layer, sorted (03 §4.2), so a renderer drawing one floor finds that floor's decoration. The version-1 layouts changed in place: the format was unreleased and no compiled chart existed outside tests. Strokes are convex pieces the compiler builds and fans itself (03 §3.1.1); only fills go through `lyon`.
- **Why:** Without a floor, a multi-floor chart would draw every floor's stage and labels at once. `lyon`'s stroke tessellator overlaps triangles at joins (darker joins when translucent) and its round joins use platform trigonometry; filling the union of overlapping stroke pieces with `lyon` failed its own coverage check at real coordinates (≈ 30,000 cu, where `f32` steps by 0.004 cu) because the pieces share near-collinear edges.
- **Rejected:** per-floor tables of decoration (a second index for the same thing); `lyon`'s stroke tessellator; filling the union of overlapping stroke pieces.
- **Cost:** a join whose segments are too short for it keeps butt ends and overlaps on its inner side. Only butt caps and miter or bevel joins.
- **Revisit if:** designers need round caps or joins, or dashes.

## D-26 · Compiled charts are served by content hash alone, brotli or raw (2026-11-11)

- **Decision:** The compiled chart URL is `/v1/public/charts/{hash}.ccf`, without the chart key and without a credential; the server finds the file by the public artefact's content hash, in any workspace. It stores the brotli form at publish time and serves it or the raw file; there is no gzip form. Versions are deduplicated per chart by the full artefact's hash (migration 2), so two versions that differ only in full-only data share one public file.
- **Why:** 03 §4.6 and 08 §6.2 had `/v1/public/charts/{chart_key}/versions/{hash}.ccf` under the publishable key, but chart keys are unique only within a workspace, a credential in a header defeats CDN caching and forces a CORS preflight, and a key in the URL changes the URL on every rotation. A content hash names identical bytes wherever they come from, and the file is public to every buyer of the event anyway. Every browser that runs the renderer accepts brotli, so a gzip form would add a column and a compressor for no client. The schema's `UNIQUE (chart_id, hash)` on the public hash would have refused a version that only changed oids, ranks or section priorities.
- **Revisit if:** private charts (signed URLs, P2) are built, or a supported client turns out not to accept brotli.

## D-27 · The public snapshot is re-encoded at each delta publication (2026-11-11)

- **Decision:** The actor re-encodes the public snapshot only when it publishes a delta, at the delta's `to_seq`, instead of after each commit at most every 50 ms (08 §4.5). Snapshot and deltas share one publication interval (100 ms, stretched with subscribers), with the first commit after a quiet interval published at once. Readers share the snapshot through an `Arc` swapped under a `std::sync::RwLock` instead of `ArcSwap`.
- **Why:** A DELTA is applied only by a receiver whose `seq` is its `from_seq` (08 §8.6). A snapshot taken between two delta boundaries has a `seq` no delta starts from, so a client that loads it over HTTP (or from a CDN, 08 §8.5) and then subscribes could never resume through the ring buffer and would be sent a second snapshot. Publishing both at once makes every snapshot a resume point. The snapshot is at most one interval old, and with a CDN in front it is cached for a second anyway. A lock held only to swap or clone an `Arc` costs readers tens of nanoseconds and needs no crate.
- **Rejected:** a 50 ms snapshot with 100 ms deltas, splitting deltas at each snapshot (twice the frames for every subscriber); clients applying a delta whose range contains their `seq` (it is wrong once changes the public cannot see are dropped, and changes the client rule); `arc-swap`.
- **Revisit if:** a snapshot one interval old is too stale for a use, or the read lock shows up in snapshot request latency.

## D-28 · Size gates: a committed payload baseline instead of a label; the chart corpus is pinned by hash, not compiled in CI (2026-11-11)

- **Decision:** The renderer payload gate (13 §6.3) compares the measured size with `xtask/payload-baseline.json`. Growth beyond 3% fails the build; a pull request that means it updates the file with `cargo xtask build-web --update-baseline`. A build without `wasm-opt`, or with another `wasm-opt` version than the baseline's, is refused. The synthetic corpus's compiled sizes (13 §4) are recorded in 03 §4.5 and guarded by each preset's pinned artefact hash in `cavea-testkit`'s tests, not by a CI step.
- **Why:** 13 §6.3 asked for "+3% without a justification label". A label needs a workflow that reads pull-request metadata, and the label carries no number; a committed baseline puts the new size in the diff the reviewer approves, and needs nothing beyond the existing `build-web` step. Sizes from an unoptimised build are about 20% larger and from other `wasm-opt` versions merely different, so comparing either would pass or fail by accident. 13 §4 had the corpus compiled in CI; CI builds in debug, where compiling and compressing the 100,000-seat preset takes 10 to 20 seconds, while the pinned hashes already fail on any change to a preset or to the compiler's output, which is when sizes need measuring again.
- **Rejected:** a GitHub label read by a workflow (another workflow, and no number); warning instead of failing above 3% (a warning in a passing CI run goes unread); building the CLI in release in CI for the corpus (minutes of build time).
- **Cost:** a deliberate increase needs a one-line change to a shared file, which conflicts when two such pull requests land together; the later one rebases and measures again. A corpus size regression is caught when the hashes are re-pinned, by the author, not by CI.
- **Revisit if:** the compiled sizes approach their targets, or CI gains a release build of the workspace for another reason.
