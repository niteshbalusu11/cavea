# 13 · Testing and benchmarks

**Status:** Draft v0.1 · 2026-11-10

Two things must be demonstrably true before anyone trusts this product with a sale: seats are never double-booked, and the renderer is as fast as we say. This document specifies how both are proven.

## 1. Principles

1. **The seat engine is tested hardest.** It gets model-based tests, simulation with injected faults, load tests and a reconciliation tool.
2. **Every performance claim has a reproducible benchmark.** A number without a script and a device does not go on the website.
3. **Decoders are fuzzed.** Anything that parses bytes from outside is treated as an attack surface.
4. **Shared logic is tested once and checked for parity** between the WASM and native builds.
5. **Tests that need a GPU or a phone are scripted** so that running them is a command, not a ritual.
6. **Every test must earn its place.** A test exists because it guards behaviour that matters and could plausibly break. Do not write tests that restate the implementation, exercise trivial getters, or exist to raise a coverage number.

## 2. Test map

| Component | Unit | Property | Fuzz | Golden image | End-to-end | Load |
|---|---|---|---|---|---|---|
| `cavea-core` (rules, best-available, hit test) | ✓ | ✓ | | | | |
| `cavea-format` (CCF, wire) | ✓ | ✓ | ✓ | | | |
| `cavea-compiler` | ✓ | ✓ | ✓ | ✓ (via renderer) | | |
| `cavea-render` | ✓ | | | ✓ | ✓ | benchmark |
| `cavea-engine` | ✓ | ✓ | | | | ✓ |
| `cavea-server` | ✓ | | ✓ (HTTP) | | ✓ | ✓ |
| `cavea-edit` | ✓ | ✓ | | ✓ | ✓ | |
| Web SDK | ✓ | | | | ✓ | |
| Console | ✓ | | | | ✓ | |
| Mobile SDKs | ✓ | | | ✓ | ✓ | benchmark |

## 3. Seat engine

### 3.1 Model-based property tests

- A **reference model**: the simplest possible implementation of the state machine, using hash maps and no batching.
- The test generates random command sequences: create hold, hold, release, book, change status, assign channel, best-available, extend, and "advance time".
- Each sequence runs against both the model and the real engine (with an in-memory store). After every command the test compares the reply and the full state, and checks invariants INV-1 to INV-10 (`02-domain-model.md` §13).
- Failures shrink to a minimal sequence and are saved as regression cases.

### 3.2 Deterministic simulation

A harness runs the actor, the committer and a simulated store on a simulated clock, driven by one random seed.

Faults injected:

| Fault | Expectation |
|---|---|
| Commit fails | Callers in that batch get an error; nothing they did is visible afterwards; state reloads correctly |
| Commit succeeds but the reply is lost (crash after commit) | After reload the state includes the batch; an idempotent retry returns the original result |
| Crash between apply and commit | The batch is not visible after reload |
| Slow commit | Batches grow; order is preserved; no reply is released early |
| Two nodes, lease expiry, takeover | The old node's next commit fails the fencing check; no command is applied twice or lost after acknowledgement |
| Clock jumps | Holds expire consistently; no hold is both used and expired |
| Duplicate request delivery | Idempotency keys make it harmless |

- Every run is reproducible from its seed.
- Per pull request: thousands of short runs. Nightly: millions of steps.
- INV-5 to INV-8 and INV-11 are checked continuously during simulation.

### 3.3 Reconciler

`cavea verify --event <key>` replays `event_log` from the beginning and compares the result with `object_state`, `holds` and `area_alloc`.

- Used after every load and chaos test.
- Shipped to customers, who can run it after an incident.
- Exit code and a machine-readable report.

### 3.4 Selection rules and best-available

- Table-driven cases for orphan rules (strict and lenient), consecutive seats and group selection, including segment edges and existing orphans.
- Properties for best-available: the result contains only eligible objects; it is contiguous whenever a contiguous option exists; no other window has a lower cost; with `prevent_orphans`, it creates an orphan only when unavoidable.
- **Parity:** the same randomly generated inputs go through the native build and the WASM build; outputs must be identical.

## 4. Format and compiler

- **Round trip:** generate a random valid chart document, compile, decode, and compare the decoded scene with the expansion computed directly (`round_trip_matches_direct_expansion` in `cavea-compiler`, 60 generated documents).
- **Determinism:** the same document produces identical bytes on Linux, macOS and Windows, and in the WASM build. The compiler's `compiled_bytes_are_deterministic` and the presets' tests pin public-artefact hashes. `cargo xtask determinism` builds the release library tests of `cavea-compiler` and `cavea-testkit` for the host and for `wasm32-unknown-unknown`, runs the first natively and the second under bun, and passes only if both legs pass. A wasm32 test binary prints nothing and takes no arguments: it runs every test that is not ignored and traps on the first failure, so the command first checks that each binary contains the pinned tests, and fails if bun, the target or a binary is missing. What it proves: x86-64 Linux and wasm32 (as bun's JavaScriptCore runs it) compile the pinned documents (two compiler documents and the six presets, up to 500,000 seats) to the same bytes. What it does not prove: macOS, Windows and aarch64, which nothing here can run, and other WASM engines. Those are covered by running the same command on each platform (natively it needs only cargo; the wasm32 leg needs bun), by hand before a release or in a CI matrix if the owner adds one; the compiler's own sine and cosine (`detmath`) and correctly rounded operations are what make agreement expected rather than lucky. Run it before filing any change to the compiler, the CCF writer or the presets.
- **Validation:** one test per error and warning code, plus a property that a document passing validation always compiles.
- **Migration:** every past schema version has stored sample documents that must migrate and compile.
- **Size:** the synthetic corpus is compiled with `cavea compile` and its sizes are recorded against the targets in `03-chart-format.md` §4.5. Each preset's public artefact hash is pinned in a test, so any change to a preset or to the compiler's output fails it, and the sizes are measured again in the pull request that re-pins them. The corpus is not compiled in CI: CI builds in debug, where compiling and compressing the 100,000-seat preset takes 10 to 20 seconds, and the hashes catch every change the sizes would (`DECISIONS.md` D-28).

## 5. Fuzzing

| Target | Input |
|---|---|
| `ccf_decode` | Arbitrary bytes into the CCF reader |
| `wire_decode` | Arbitrary bytes into the realtime frame decoder |
| `document_parse` | Arbitrary JSON into the chart document parser and compiler |
| `snapshot_apply` | A valid chart plus arbitrary snapshot and delta bytes into the engine |
| HTTP | Requests generated from the OpenAPI document against a running server |

- Seed corpora are kept in the repository.
- Nightly: 30 minutes per target. Any crash, panic or hang is a release blocker.
- The code under fuzzing is compiled with overflow checks and debug assertions on. libFuzzer runs native code only, so the targets fuzz the same crates the WASM build ships, natively.

**What exists.** The crate `fuzz/` holds four `cargo-fuzz` (libFuzzer) targets. It is its own cargo workspace with its own lockfile and target directory, so the main workspace's build, lint and CI never see it; it needs a nightly compiler, which the second dev shell `nix develop .#fuzz` provides with `cargo-fuzz` (the default shell keeps the pinned stable toolchain). Its release profile turns on overflow checks and debug assertions; `cargo fuzz` adds AddressSanitizer.

| Target | Property checked on every input |
|---|---|
| `ccf_decode` | Bytes into `decode` as both artefacts. An accepted chart passes `Chart::check`, re-encodes, re-decodes to the same chart, and re-encodes to the same bytes again. A file accepted as the full artefact is accepted as the public one with the same chart less the full-only data |
| `document_parse` | Bytes into `ChartDocument::from_json`; a parsed document saves and loads back unchanged, then compiles. A refusal lists errors only; a compiled chart carries warnings only, both artefacts decode to the same chart, and their stored content hashes are their BLAKE3 hashes |
| `wire_decode` | Bytes into `decode_server` (public and manager views) and `decode_client`. Accepted frames: codes within the view, every `idx` strictly increasing and below the object count, a receiver with fewer objects refused, a snapshot's runs covering exactly its objects; re-encoding the decoded values and decoding again gives the same values |
| `snapshot_apply` | A fixed 102-seat, two-floor chart and a fuzzer-chosen sequence of chart loads, server frames (through `Command::from_frame`), selections, hold results, session modes and frames, into the renderer engine on a backend that asserts every buffer and texture write and every drawn instance range lies inside the resource it created |

Every target is bounded in time by `-timeout=10` (a hang is a finding) and in memory by libFuzzer's default 2,048 MB limit. Seeds are `fuzz/corpus/<target>/seed-*`, 31 files and 54 KB in all: the compiler's stored schema samples, the fixed chart and a 240-seat synthetic stadium as documents and as both artefacts, refusals the parser must give cleanly, one valid frame of each type, and four engine scenarios. `cargo run --example seeds` in the fuzz shell writes them again after a format change. libFuzzer adds the inputs it finds to the same directory under hash names, which are gitignored, as are `fuzz/artifacts/` (crashes) and `fuzz/target/`.

**How to run.** From the repository root, one target for `n` seconds on one core:

```sh
nix develop .#fuzz -c cargo fuzz run ccf_decode -- -max_total_time=n -timeout=10
```

The 24-hour run of the CCF decoder for the M1 exit criterion is the same command with `-max_total_time=86400`. A finding stops the run and leaves the input in `fuzz/artifacts/<target>/`; `cargo fuzz run <target> <file>` reproduces it and `cargo fuzz tmin <target> <file>` minimises it. Nothing runs fuzzing nightly yet: there is no nightly job (CI runs no tests by decision).

**Runs so far** (local, on the development machine, AMD Ryzen 9 7900X, one core per target, the four at once):

FUZZ_RUNS_TABLE

## 6. Renderer

### 6.1 Correctness

- **Golden images:** the headless mode renders fixed charts, states and cameras; results are compared with stored images using a per-pixel tolerance and a cap on the share of differing pixels. Covered: each level of detail, each visual state, each seat shape, each theme, text in Latin and non-Latin scripts, right-to-left.
- **Backend parity:** the same scenes rendered by the `glow` and `wgpu` backends must match within tolerance. The Canvas 2D fallback is compared with a looser tolerance.
- **Interaction:** scripted pointer and key traces must produce the expected sequence of engine events.
- **Fallback ladder:** tests run with WebGL2 disabled and with WASM blocked.
- **Context loss:** the test forces a lost context and checks the picture after restoration.

### 6.2 Benchmark methodology

**Corpus:** the synthetic presets of `03-chart-format.md` §7.

**Scenarios**

| Id | Scenario | Measures |
|---|---|---|
| S1 | Cold load to chart-ready | Timing marks, payload bytes |
| S2 | 30-second scripted gesture trace: overview, zoom into a section, pan along rows, zoom out | Frame time distribution |
| S3 | S2 while applying 1,000 status changes per second | Frame time distribution, delta apply time |
| S4 | 50 taps on seats | Input to next frame |
| S5 | After S2 | Memory |

**Metrics:** chart-ready time; frame time median, 95th and 99th percentile, maximum; share of frames over budget; payload size; WASM memory; estimated GPU memory.

**Network profiles**

| Profile | Down | Round trip |
|---|---|---|
| Cable | Unthrottled | — |
| 4G | 9 Mbit/s | 170 ms |
| Slow 4G | 1.6 Mbit/s | 300 ms |

**Devices:** the four reference devices of `01-architecture.md` §5.

**Procedure**

- Ten runs per scenario and device; report the median and the 90th percentile.
- Cold-cache runs clear storage and caches before each run.
- Desktop runs are automated with Playwright and browser tracing.
- Phone runs use the benchmark page with a built-in harness that posts results to a collector on the local network.
- Native runs use the example apps with the same trace.
- Record browser version, OS version, device model and battery or thermal state with every result.

**Publishing**

- Results live in `bench/results/` with the commit they were measured on.
- The public benchmark page shows our numbers, the method and the scripts.
- Comparisons with competitors are published only if their terms allow benchmarking, only against their public demos, and always with the method. Otherwise we publish our numbers alone next to their own published claims.

### 6.3 Regression gates

| Gate | Where | Fails when |
|---|---|---|
| Payload size | Every pull request | Over the hard cap, or more than 3% over the committed baseline |
| Micro-benchmarks (decode, hit test, delta apply, label build, best-available) | Every pull request | More than 10% slower than the baseline |
| Headless frame-time smoke | Every pull request | Soft warning only; software rendering is not representative |
| Device benchmarks S1–S5 | Every release | Any budget in `01-architecture.md` §5 is missed |

**Micro-benchmarks.** `cavea bench` (`cargo run --release -p cavea-cli -- bench [name…]`) measures on the `stadium-100k` preset: CCF decode; hit test (10,000 taps near seats at seat sizes of 18 to 64 px with the 12 px touch tolerance, time per test); snapshot apply (100,000 objects) and delta apply (10,000 changes), each from frame bytes through `Command::from_frame`, the engine and the next frame's buffer upload, on a backend that does nothing; seat labels of 4,096 seats (a full render chunk's worth) through `cavea_format::full_label::seat_label`; and compiling each preset. Each benchmark warms up for 0.2 s, then takes 5 to 15 samples of at least 10 ms each (short runs are batched) and reports the median and the minimum time per run, with the budget beside it where one has a hard number: decode 5 ms per 100,000 seats (`03-chart-format.md` §4.4), hit test 20 µs (`04-renderer.md` §15), delta apply 2 ms (`01-architecture.md` §5), label build 1 ms (`04-renderer.md` §14, "well under a millisecond"). A missed budget exits 4. `--json` prints the same as JSON. It refuses to run in a debug build.

The gate compares a machine with itself: `--save <file>` writes the medians, and `--baseline <file>` compares a later run and exits 4 when any benchmark's median is more than 10% slower. A builder runs it with `--save` on `main` and `--baseline` on the branch before filing a change to the measured code; a flagged result is re-run before it is believed (the median of 5 to 15 samples absorbs most noise, but not another process taking the core). No baseline is committed and CI runs none: a number from one machine says nothing about another.

Measured locally, 2026-11-11, AMD Ryzen 9 7900X, Linux, release build, with other builds running on the machine; not reference-device results:

BENCH_TABLE

**Payload gate.** `cargo xtask build-web` measures the renderer payload, the brotli size (quality 11) of the production JavaScript glue plus the WASM after `wasm-opt -Oz` (plus the font atlas once there is one), and compares it with `xtask/payload-baseline.json`, which records the last accepted size and the `wasm-opt` version it was measured with. The build fails when the payload is over the hard cap of 200,000 B (`01-architecture.md` §5), or more than 3% over the baseline. It refuses to compare a build made without `wasm-opt` (about a fifth larger) or with another `wasm-opt` version than the baseline's: either is an error, not a pass. CI installs binaryen 132, the version `flake.nix` provides; upgrading binaryen changes the flake, CI and the baseline together. The `--demo` build is never measured.

The justification is the baseline file itself, not a label: a pull request that grows the payload by more than 3% on purpose runs `cargo xtask build-web --update-baseline` and commits the file, so the new size is in its diff and its description says why. Growth under 3% passes without an update, and so does a decrease; growth then adds up against the same baseline until it crosses 3%. Refresh the baseline in any pull request that moves the payload more than 3% either way (the build says so when it shrinks), and when the orchestrator asks after another pull request has changed it.

## 7. Server load and chaos

### 7.1 Load generator

`cavea-loadgen`, written in Rust, reusing the wire codecs. It simulates buyers: connect, receive the snapshot, pick seats (random, clustered or best-available), hold, optionally release, and a "backend" that books a share of holds.

### 7.2 Scenarios

| Id | Scenario | Pass criteria |
|---|---|---|
| L1 | **On-sale stampede:** 50,000 subscribers; 5,000 buyers each try to hold 2–4 seats within the first seconds on a 20,000-seat event | No double booking (reconciler); hold 99th percentile within budget; every refusal has a correct reason |
| L2 | Sustained 2,000 holds per second for 10 minutes | Latency within budget; memory flat |
| L3 | Expiry wave: 20,000 holds expiring within one second | All released; deltas delivered; no stall |
| L4 | Reconnect storm: restart the server with 50,000 subscribers | All resubscribe within 30 s; no overload collapse |
| L5 | Best-available contention: 1,000 concurrent requests for 4 seats | Disjoint results; no orphan created where avoidable |
| L6 | Soak: mixed traffic for 24 hours | No memory growth; no error drift |

Hardware: a rented 4-vCPU machine plus a PostgreSQL instance, recorded with the results.

### 7.3 Chaos

| Experiment | Check |
|---|---|
| `kill -9` the server during L2 | Reconciler passes; acknowledged commands are present |
| Restart PostgreSQL during L2 | Errors during the outage; automatic recovery; reconciler passes |
| Inject 200 ms latency and packet loss between server and database | Batches grow; no timeouts cascade |
| Fill the database disk | Clean failure; recovery after space is freed |
| Kill the active node in an active–passive pair (P2) | Standby takes over within the lease period; reconciler passes |

## 8. Compatibility

| Dimension | Matrix |
|---|---|
| Browsers | Chrome, Edge, Firefox, Safari: current and previous two major versions; iOS Safari 15+; Samsung Internet |
| PostgreSQL | 15, 16, 17, 18 |
| Server platforms | Linux x86-64 and arm64 |
| iOS, Android | Minimum versions in `06-mobile-sdk.md` §4, and the latest |
| Bundlers | Vite, webpack 5, Rollup, esbuild, and a plain script tag |
| Frameworks | React 18 and 19, Vue 3, Svelte 5, Next.js (server rendering) |

Browser coverage uses Playwright locally and a device cloud for what it cannot reach.

## 9. Designer

- **Command inverses:** for random command sequences, applying then undoing every command returns the original document, byte for byte.
- **Tools:** scripted pointer traces produce expected documents.
- **End-to-end (Playwright):** draw a block, curve it, number it, assign a category, publish, create an event through the API, hold a seat, and see it in the event manager.
- **Performance:** scripted drag of 5,000 seats on the 100,000-seat chart, measured as in §6.2.

## 10. Security tests

- **Authorisation matrix:** a generated test calls every endpoint with every credential type and with another workspace's identifiers, and compares the status with a declared table.
- **Tenant isolation:** two workspaces with identical keys for charts and events; no call may cross.
- **Dependency audit** on every pull request.
- **Secrets:** a scan for committed secrets in CI.

## 11. Accessibility tests

Specified in `11-accessibility.md` §9.

## 12. Continuous integration

| Stage | Contents | Budget |
|---|---|---|
| Every pull request | In CI: format, lint, type checks and builds, including the WASM build with size gate. Run locally by the author before filing: unit tests, short property and simulation runs, micro-benchmarks, end-to-end smoke, authorisation matrix | About 10 minutes |
| Nightly | Long simulation, fuzzing, full golden images, L2 for two minutes, full end-to-end | About 2 hours |
| Every release | Device benchmarks, L1–L5, chaos, upgrade test from the previous release with data, compatibility matrix, manual accessibility matrix | About a day |

## 13. Release checklist

- [ ] All pull-request and nightly suites green on the release commit.
- [ ] Device benchmarks within every budget; results committed.
- [ ] L1 and L2 pass; reconciler clean.
- [ ] Chaos experiments pass.
- [ ] Upgrade from the previous release tested with a populated database.
- [ ] Renderer of the previous two minors works against the new server.
- [ ] Changelog and upgrade notes written; breaking changes called out.
- [ ] Docs updated; API reference regenerated.
- [ ] Images and packages signed; SBOM attached.
