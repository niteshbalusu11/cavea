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

- **Round trip:** generate a random valid chart document, compile, decode, and compare the decoded scene with the expansion computed directly.
- **Determinism:** the same document produces identical bytes on Linux, macOS and Windows, and in the WASM build.
- **Validation:** one test per error and warning code, plus a property that a document passing validation always compiles.
- **Migration:** every past schema version has stored sample documents that must migrate and compile.
- **Size:** the synthetic corpus is compiled in CI and sizes are compared with the targets in `03-chart-format.md` §4.5.

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
- The WASM build is compiled with overflow checks in the fuzz configuration.

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
| Payload size | Every pull request | Over the hard cap, or +3% without a justification label |
| Micro-benchmarks (decode, hit test, delta apply, label build, best-available) | Every pull request | More than 10% slower than the baseline |
| Headless frame-time smoke | Every pull request | Soft warning only; software rendering is not representative |
| Device benchmarks S1–S5 | Every release | Any budget in `01-architecture.md` §5 is missed |

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
