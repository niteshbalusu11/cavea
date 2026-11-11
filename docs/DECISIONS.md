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

## D-29 · REN-019 result: `naga` translates the `mesh` program too; section tints are vertex colours (2026-11-11)

- **Result:** D-05 and D-22 hold for the `mesh` program. `shaders/mesh.wgsl` (position `vec2<f32>` and colour `vec4<f32>` in, colour × the block's `alpha` out, premultiplied) goes through the same `build.rs` path with no change to it: `naga` 30.0.1 writes GLSL ES 3.00 that compiled and linked in Chromium 157 (ANGLE on SwiftShader, software rendering) and drew decoration and section fills with `gl.getError() == 0`. The colour input is fed from RGBA8 with a normalised `vertexAttribPointer`; indices are `u32` (`drawElements` with `UNSIGNED_INT`, core in WebGL2). The fragment stage uses no uniforms, so `naga` emits the block in the vertex stage only (`Globals_block_0Vertex`), and `build.rs` emits only that name. Both programs read one shared uniform buffer at binding point 0. No hand-written fallback was needed.
- **Section tints are written into the section outlines' vertex colours** (white in `MESH`, 03 §4.2.3) instead of a per-draw tint uniform: the active floor's outlines are one index range, so all section fills are one draw call (a stadium has about 120 sections), and no vertex attribute or texture is added. A tint change uploads that section's vertex span.
- **Still not proven:** `glyph`, `text` and `image`, and a hardware GPU or another browser engine.

## D-30 · Seats keep their stroke from 5 px, not only at the detail level (2026-11-11)

- **Decision:** A seat's stroke is drawn from 5 px on screen by its own size, the start of the seats level, as 04 §8 already said; the detail level (14 px) adds seat labels and icons, not the stroke. 04 §10's table had "flat seats, no stroke" between 5 and 14 px.
- **Why:** Unavailable seats are filled close to the background and drawn hollow by their stroke (04 §19). Without it, between 5 and 14 px, the most used zoom range, available and unavailable seats would differ by lightness alone (`11-accessibility.md` §6 asks for more than colour). A per-seat rule also handles rows whose seats are larger than the chart's default.
- **Cost:** At small sizes the 1.5 px stroke is a large share of a seat.
- **Revisit if:** a device study shows strokes at 5 to 14 px cost frame time or legibility.

## D-31 · Realtime routes take the publishable key in the URL (2026-11-11)

- **Decision:** `GET /v1/public/events/{event}/state`, `…/stream` and `/v1/public/rt` take the publishable key in the `key` query parameter instead of `Authorization` (08 §6.2, §8.2, §8.5, §11). The key names the workspace, so the snapshot URL is a complete CDN cache key; a cached snapshot may reach a page the key's origin allow-list would refuse, or outlive a revoked key by a few seconds. The hold token stays out of every URL: the WebSocket carries it inside SUBSCRIBE.
- **Why:** browsers cannot set headers on a WebSocket or an `EventSource`; event keys are unique only within a workspace, so the URL must name the workspace for a shared cache to keep one event's snapshot apart from another workspace's (D-26 had the same problem for charts); a credential in a header stops shared caching and forces a CORS preflight. A publishable key is public (it is in every page that embeds the renderer), and the public snapshot reveals only which seats are unavailable (D-12).
- **Rejected:** the key in `Authorization` with `Vary: Authorization` (most CDNs do not cache it); the workspace in the path (a workspace would come from the request, against 08 §11); the key in `Sec-WebSocket-Protocol` (WebSocket only, and misuses the header); the key in a first frame (SUBSCRIBE has no field for it, and the origin could then only be checked after the upgrade).
- **Revisit if:** private or channel-specific snapshots are built (they are `private`, 08 §8.5), or a customer needs the allow-list enforced on cached reads.

## D-32 · REN-024 result: `naga` translates the `glyph` program; ink is read from the seats' dynamic buffer (2026-11-11)

- **Result:** D-05, D-22 and D-29 hold for the `glyph` program. `shaders/glyph.wgsl` goes through the same `build.rs` path unchanged: `naga` 30.0.1 writes GLSL ES 3.00 that compiled and linked in Chromium 155 (ANGLE on SwiftShader, software rendering) and drew seat labels with `gl.getError() == 0`. New in this program and translated as expected: a second uniform block (`Font_block_1Vertex`, the glyph metrics, `array<vec4<f32>, 64>` indexed by a per-instance code), `vec4<u32>` and `vec4<i32>` vertex inputs fed from `u8` and `i8` with `vertexAttribIPointer`, dynamic indexing of a vector by the slot, `textureSample` with a sampler (a `sampler2D` in the fragment stage, on texture unit 0) beside `textureLoad` of the palette in the vertex stage (unit 1). The GL backend binds blocks and samplers by the names `build.rs` emits, the globals at binding point 0 and the metrics at 1, uploaded once at start. No hand-written fallback was needed.
- **Ink follows state through the instance's `idx`, not through the label buffer.** Each seat's label is one instance; the draw binds the label buffer from the chunk's slot and the seats' dynamic buffer (category, visual state, effects) at the chunk's first `idx`, so the program fetches the ink texel exactly as the `shape` program fetches fill and stroke (04 §8, §9). `DrawCall` gained a second texture and, per range, the label buffer instance where it starts (`bases`). A status change, a selection or a theme change uploads nothing to label buffers, and dimming applies to labels as to seats.
- **Rejected:** a colour index per glyph instance (labels rebuilt on every hold and delta); a copy of the seats' state in a texture indexed by `idx` (a second upload on every state change); per-glyph instances with the seat's attributes at a divisor of four (40 to 48 bytes a seat, over the 6 MB budget for 64 chunks of 4,096 seats); glyph metrics in each instance (the same); a third texture for the metrics (a constant uniform block needs no upload path and no texture unit).
- **Anti-aliasing:** coverage is the distance in device pixels, the atlas's range per em times the label's em on screen, computed once per label in the vertex stage. Labels are upright and scaled uniformly, so this is exact; screen-space derivatives would cost a fragment-stage computation and are coarse (2 × 2 pixel blocks) on some mobile GPUs.
- **Still not proven:** `text` and `image`, and a hardware GPU or another browser engine.

## D-33 · The glyph atlas: Roboto Condensed Medium, 40 glyphs, 16 px per em, a browser-decoded PNG baked by a pinned Nix shell (2026-11-11)

- **Decision:** The baked atlas (D-18) holds 0–9, A–Z, space, `-`, `.` and `/` of Roboto Condensed Medium 3.016 (SIL OFL 1.1), as a multi-channel signed distance field at 16 px per em with a 3 px range: a 92 × 92 PNG of 9,458 bytes. The host fetches it with the WASM and decodes it with `createImageBitmap`; the engine receives RGBA8 pixels with `SetGlyphAtlas`; the metrics are compiled into the WASM. `tools/glyph-atlas` bakes it with `msdf-atlas-gen` 1.4, built from source in a pinned `nix-shell` (nixpkgs has no MSDF generator), after removing the font's overlapping contours with fontTools; the baked files are committed and checked by a test.
- **Why:** The atlas is the largest single item in the renderer's payload (01 §5). Measured (04 §12): 256 × 256, the size first planned, is 41 KB for this set and 62 KB for printable ASCII; this set at 16 px per em is 9.5 KB and reads correctly from the 7 px minimum to about 110 px per em, the largest a label reaches (64 px seats at a ratio of 3), with small notches at the junctions of `3`, `4`, `A` and `B` from about 60 px per em; 24 px per em (16.1 KB) would remove most of them, for 6.6 KB more. Seat labels in the synthetic presets are digits only, and digits plus capitals cover nearly every seat label; lowercase and other punctuation are row-label and price text, which the dynamic atlas can draw. A PNG is the smallest form tried (raw RGB8 plus brotli was 4 to 15% larger) and needs no server configuration and no decoder in the WASM; embedding the bytes in the WASM would have delayed the first frame (04 §4). Condensed digits (0.5 em, tabular) let labels be about 9% larger in a round seat than Roboto's regular width.
- **Rejected:** a generator crate in `xtask` (the pure-Rust `fdsm` brings `nalgebra` and `image` into every CI build); a single-channel distance field (4 KB at the same size; it rounds corners under magnification, why D-18 chose a multi-channel field; not compared further); Roboto at its regular width (digits 0.568 em).
- **Cost:** No lowercase in the baked atlas. Rebaking needs Nix and a minute to build the generator. Roboto 3's licence must ship with the renderer: `packages/renderer/assets/OFL.txt` sits beside the atlas, and a product built on Cavea must carry the font's copyright and licence notice.
- **Revisit if:** seat labels with lowercase are common in real charts; the notches at high zoom are judged worth 6.6 KB (24 px per em); or the payload budget tightens enough that the atlas's 9.5 KB matters.

## D-34 · Session tokens are HS256 keyed by a secret key, verified with its stored SHA-256 (2026-11-11)

- **Decision:** A session token (08 §11) is a JWT signed with HS256; its `kid` header names one of the workspace's secret keys, and the HMAC key is that secret key. A secret key is 67 bytes, longer than SHA-256's 64-byte block, so HMAC uses its SHA-256 in its place (RFC 2104 §2): the 32 bytes the server already stores in `api_keys.hash` verify the token, and a backend can sign with the key text as any JWT library expects, or with its SHA-256. A revoked key no longer verifies; rotation is signing with the new key's id. No new secret, storage or endpoint.
- **Why:** 08 §11 had "signed by the customer's backend with a secret key (HS256)", but the server keeps only key hashes, so it could not have verified an HMAC keyed by anything it does not hold. Keying by the hash needs nothing new on either side.
- **Cost:** a stored key hash, harmless until now (a key is found by its hash, and the hash does not authenticate), becomes enough to forge session tokens if the database leaks. Accepted: session tokens gate seat holding, the anti-hoarding defence, not booking or anything a secret key does; a forged session still holds only within each token's object limit, the per-`sub` cap and the rate limits.
- **Rejected:** a separate signing secret per key or workspace, shown once like a key. Stored in the same database it leaks in the same breach, so it only helps if it is encrypted with a key kept outside the database (a KMS or a server-side key in configuration), which is new key management for every deployment. Asymmetric tokens (RS256, EdDSA) with a registered public key: safe against a leak, but a key upload and management endpoint, and the customer runs key generation.
- **Revisit if:** session tokens come to authorise more than holding (channels that gate inventory a customer sells separately, or booking), or a deployment needs a database leak not to let anyone mint sessions; then a per-workspace signing key encrypted outside the database, or asymmetric keys, with a `kid` of their own.

## D-35 · Web SDK: options not built yet are left out of the types; development means `NODE_ENV` is not `"production"`; the payload gate bundles the SDK (2026-11-11)

- **Decision:** Options, callbacks and methods of 05 that are not built are absent from `@cavea/renderer`'s TypeScript types; passed or called anyway they are ignored with a console warning in development builds (05 §3.6). A development build is one where `process.env.NODE_ENV` is not `"production"` (a page without `process` counts as development). The state adapter interface of 05 §4 replaces the sketch (`subscribe` and `seq` dropped; `chart`, `resync`, `pause`, `resume`, `setHoldToken`, `applyFrame` and per-object hold reasons added). The payload gate measures a minified bundle of the public entry, built by bun in CI's `rust` job, instead of the raw wasm-bindgen glue.
- **Why:** A type that accepts `pricing` while the map ignores it would let a customer's typed build claim a feature the package lacks; leaving it out makes the compiler say so, and the runtime warning covers untyped callers. `NODE_ENV` is what every bundler replaces, so production warnings cost nothing and need no option. The sketch could not express what `caveaServer` needs (a chart location before the snapshot, binary frames untouched, the hold token, resync and visibility), and its `seq` duplicated the frame's. The SDK's JavaScript is part of the 150 KB budget (01 §5) and was unmeasured; a bundle counts what a buyer downloads, with the glue minified as it ships. Bun in the job that already gates is one setup step; moving the gate to the `web` job would split the WASM and JavaScript checks across jobs.
- **Rejected:** typing unbuilt options as present but deprecated or `never` (still accepted by object spreads and untyped wrappers, and noisier than absence); a `debug` option (one more option to validate, and production warnings would stay in the bundle); a bun script in the `web` job adding the bundle to the Rust gate's numbers (two places to read a size from).
- **Revisit if:** customers ask to silence development warnings without a production build, or the SDK bundle passes 20 KB brotli.

## D-36 · `caveaServer`: resync on the stream, lazy hold tokens, a transport that worked is kept, the clock offset in the adapter interface (2026-11-11)

- **Decision:** The renderer's server adapter (05 §4.1) resyncs by subscribing again with `seq` 0 (polling: asking from 0), so the SNAPSHOT arrives on the stream in order with the deltas after it, instead of fetching `…/state` over HTTP. In `start` mode it creates the hold token at the first tap, not on load, and replaces a token that has ended with a new one within the same hold. `auto` gives a transport up only after two consecutive failures with no HELLO ever on it, and never goes back to a better one. It reconnects SSE itself after every error, with jittered backoff, instead of leaving dropped connections to `EventSource`. The adapter context gains `setClockOffset`, and `Selection.expiresAt` is on the page's clock.
- **Why:** An HTTP snapshot can be up to six seconds old behind a cache (08 §8.5) and older than the stream's position, so the deltas after it never line up; the stream's own SNAPSHOT is in order by construction. An eager token costs every visitor who never taps a request, a server-side hold record and, with sessions, one of `max_tokens_per_sub`. A transport that once worked and then drops is a server restart or a network blip, not a blocked transport: falling back then would leave a page on polling for its life. `EventSource` retries after a fixed delay, so a restart would bring every SSE client back at once. Only the adapter knows the server's clock (HELLO, token creation), and only the map runs the expiry callbacks; a host computing a countdown from a server timestamp and `Date.now()` is wrong by the buyer's clock error.
- **Rejected:** a token created on connect (simpler first tap, the costs above); periodic retries of the better transport (flapping, and polling is rare); expiry callbacks in the adapter (every custom adapter would repeat them).
- **Revisit if:** first-tap latency matters more than the cost of idle tokens, or polling turns out to be common enough that upgrading back to a WebSocket pays.

## D-37 · The reconciler is `cavea-server verify`; the log carries a hold's limit but never its token hash (2026-11-11)

- **Decision:** The reconciler (13 §3.3) is a subcommand of the server binary, `cavea-server verify`, not `cavea verify`. `CreateHold` log rows carry the hold's object limit in `detail`, so replaying the log rebuilds every `holds` column the engine writes except `token_hash`; the reconciler verifies holds by id, expiry, limit and objects, and reports the token hash as not checked.
- **Why:** The CLI has no database driver or async runtime; adding `sqlx` and `tokio` for one subcommand would more than double its dependency tree, while the server has both, and its image is what a customer has deployed and can run after an incident with the settings it already has. The token hash is the SHA-256 of a bearer secret; `holds` keeps it only while the hold lives, but the log is kept for 400 days and exported by the reports endpoint, so logging it would spread a token's fingerprint far beyond the hold for a check that adds little: a wrong hash cannot double-book a seat, and a corrupted one makes the token unusable, which its holder notices.
- **Rejected:** `cavea verify` in the CLI (its build cost); logging the token hash (exposure for no correctness gain).
- **Cost:** operators run the reconciler through the server binary or image; a stored hash that was changed goes undetected.
- **Revisit if:** the CLI gains a database driver for another reason, or tokens need auditing after their hold ends.
## D-38 · Dynamic text crosses the engine boundary as an event and a command, not a host trait (2026-11-11)

- **Decision:** The engine asks for a string with `Event::TextRequest` (id, text, size in device pixels, weight, font family) while it builds labels, and the host answers with `Command::SetText` (id, width, height, the em it drew at, coverage bytes). The `TextRasteriser` trait of `cavea_render::host`, written for this, is removed. On the web the SDK's host draws each request on one small 2D canvas (`OffscreenCanvas` where there is one) as it takes the engine's events after a frame, and calls `View.setText`.
- **Why:** D-21: the engine is driven by commands and emits events, so it can move to a worker. A synchronous trait would be called in the middle of building a frame, so `fillText` would run between GL calls, and in a worker the canvas would have to be the worker's; with an event the host decides when and where to draw, outside the frame, and a worker host answers the same way from an `OffscreenCanvas`. Tests answer with a fake that returns fixed bitmaps, through `Engine::handle` like everything else. The cost is a frame of latency for a new string, which seat labels already accept (the atlas arrives late too), and a request id kept per pending string.
- **Bounds:** at most 16 requests a build and 64 outstanding, 4,096 strings tracked, 256 bytes a string, rasters of at most 1,024 × 512 px (the host shrinks the font to fit; larger answers are refused as `InvalidInput`). An answer to a request the atlas no longer waits for (a new chart, a new font, a cleared atlas) is ignored. A host that never answers leaves those labels undrawn and the frame loop idle.
- **Rejected:** a synchronous trait (above); measuring with one call and drawing with another (two round trips for one string; the raster's size is the measurement); the browser drawing labels in a DOM overlay (01 §5 keeps overlays few and outside the frame loop, and labels must follow the camera every frame).
- **Revisit if:** worker mode (REN-P2) finds the per-request copy of the bytes costly, or a native host wants to batch.

## D-39 · REN-026 result: `naga` translates the `text` program (2026-11-11)

- **Result:** D-05, D-22, D-29 and D-32 hold for the fourth program. `shaders/text.wgsl` goes through the same `build.rs` path with no change to it: `naga` 30.0.1 writes GLSL ES 3.00 (a `vec2` constant array, a loop with `textureLod`, `textureSize`, integer attributes from `u16`, `i16` and `u8`) that compiled and linked in Chromium 157 (ANGLE on SwiftShader, software rendering) with `gl.getError() == 0`. No hand-written GLSL was needed.
- **Sampling:** `textureSampleLevel` (explicit level 0) everywhere, because the halo samples inside a branch on a flat varying, where WGSL forbids implicit-derivative sampling; the atlas has no mipmaps, so nothing is lost.
- **Still not proven:** `image`, and a hardware GPU or another browser engine.
