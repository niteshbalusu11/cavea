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

**Implemented** in `crates/cavea-sim` (TST-020), a crate of its own so that the engine's test builds do not compile it, and so that the reconciler and the load generator can reuse the model and its log replay:

- `model::Model` is written from 02 §4 to §6 and 08 §6.1, §7 (including the order of checks, 08 §7): hash maps from object to status and from token to hold, one command at a time, expiry as its own command, and the idempotency window. `model::Replay` rebuilds state from the committed log alone and refuses any record that is not a legal transition from the state before it.
- `case::generate` weights sequences towards contention: 3 to 12 objects, four tokens, both authorities, tokens near their limits, time moved onto a hold's expiry instant and a millisecond either side, small steps, 15-minute and 24-hour jumps and small jumps back, idempotency keys replayed with the same and with another fingerprint, and hostile input (unknown labels and `idx`, duplicates, empty and 1,001-object lists, lifetimes and limits out of range, empty, long and U+0000 text, oversized `extra`, mismatched details). Commands for channels, areas and best-available wait for those features.
- After every step the reply (code, per-object reasons, `seq`, objects) and the full state (every object's status, holder and booking; every hold's id, token, expiry, limit and objects; `seq`; the next expiry) must equal the model's, and INV-1 and INV-2 hold; after a publishable command no booked object changed (INV-3). At random points the batch is sealed: its log must equal the model's, continue the stored `seq` (INV-5; the store refuses a gap), replay through `Replay` into exactly memory and the store's rows, `seq` per row included (INV-6), with no object booked twice without a release (INV-10); now and then the engine is replaced by one restored from the store. INV-4 waits for areas; INV-7, INV-8, INV-9 and INV-11 need the actor, the feed and the store over time, and belong to the simulation (§3.2). The log carries each hold's id, expiry, object limit and objects; its token hash is only in `holds` (08 §4.4), so INV-6 covers holds by everything else. The replay is `cavea_engine::Replay`, the reconciler's (§3.3).
- A failure shrinks (runs of steps dropped, then objects, details, keys and authorities, and time steps shortened, while it fails the same check) and prints a case in a line-based text format; saved under `crates/cavea-sim/cases/`, every case replays in `cargo test`.
- `cargo test -p cavea-sim` runs 1,000 sequences of 150 steps (1.4 s in a debug build). Longer: `cargo run --release -p cavea-sim --example soak -- model --seeds 1..100001 --steps 400 --jobs 4`.
- **Run locally** on 2026-11-11 (AMD Ryzen 9 7900X, 4 threads): seeds 1 to 100,000, 400 steps each, 40,000,000 steps in 10.3 s; no divergence. Four deliberate breaks of the state machine, made one at a time and never committed (an expired token still usable at its expiry instant, a fully booked token kept, a mismatched key replayed, a refusal taking a `seq`), were each caught within the first five seeds; their shrunk cases are the saved ones.

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

**Implemented** in `crates/cavea-sim` (`sim`, TST-021). Two nodes, each a real `Registry` with its actors and feeds on a current-thread Tokio runtime of its own, share one `MemoryStore` through a wrapper that holds every commit until the seed decides it: commit, fail, write but lose the reply, or panic. A node can be crashed (its runtime dropped, commits in flight with it), frozen past its lease while the other node takes the event over, cut off from the store, shut down in order mid-traffic, or have its clock jumped forward or back; clients pipeline requests (keyed ones duplicated and retried), subscribers attach, stall until they lag, resync, and resume from their `seq` after their actor fails. Some seeds use a mailbox of 6 and batches of 3, so that overload and full batches occur.

- **Every step:** the committed log replays through `model::Replay` without a gap into exactly the store's rows (INV-5, INV-6, INV-10); every batch comes from the node that acquired its epoch, and epochs never go back (INV-11); no command is logged twice, or after its batch failed or died with its node; every acknowledged reply names a committed `seq`, and an acknowledged change, or a keyed reply's record, is in the store (INV-7, and no reply before its commit); every frame a subscriber applies is checked as a renderer would (a DELTA continues its `seq`), leaves it holding the public reduction of the committed state at its `seq`, never past the store's, with a PRIVATE frame on every attach and resync and its PRIVATE equal to its token's committed hold (INV-8), and a SNAPSHOT re-encodes from public codes alone (INV-9); every caller waiting on an actor when its commit failed, panicked or was written but lost its reply (in that batch, the next, or the mailbox: keyed refusals and no-ops in a batch without log rows included) gets `unavailable`, never a success; an outage costs at most one load per event per second (08 §4.6); a shutdown releases every lease an actor that did not fail held.
- **End of run:** every fault is lifted, everything commits and every subscriber must reach the committed `seq`. Then the log replays through the model in `seq` order, each command with its own request: every logged command gives the reply its client received and the log record the store has, a refusal or no-op matches a committed state its client could have seen, between the committed `seq` when it was sent and when its reply arrived, duplicates and retries of a keyed request get the original reply and are never logged a second time, a key reused with another request gets `idempotency_mismatch`, no hold was used at or after its expiry, and each client's commands committed in the order it sent them.
- **Determinism.** Tokio's `watch` wakes its waiters in an order drawn from a per-runtime random seed, so the engine's timers sleep on a harness clock that wakes them in a fixed order, each node's clients go through one task, and a node runs each step until idle rather than for a fixed number of yields. In `cargo test` seeds 1 to 30 are each run twice; the long run prints the sum of every run's trace hash, the same in any process with any number of threads.
- `cargo test -p cavea-sim` runs 300 seeds of 100 steps (2.4 s in a debug build). Longer: `cargo run --release -p cavea-sim --example soak -- sim --seeds 1..2001 --steps 1000 --jobs 4`.
- **Run locally** on 2026-11-11 (AMD Ryzen 9 7900X, at most 4 threads): seeds 1 to 2,000, 1,000 steps each, 2,000,000 steps in 7.1 s (280,000 steps a second; 14.1 s with 2 threads); every invariant held. Trace hash sum `64ea2c58820abd3b` with 4 and with 2 threads (`1408a515723d9cdc` before hold limits were logged, which changes the hashed batches). Faults fired: 6,399 failed commits, 6,827 lost replies, 1,215 panics in a commit, 4,334 node crashes (1,495 with a commit in flight), 122,465 slow commits, 4,891 frozen nodes, 4,629 takeovers with 3,427 fenced commits and heartbeats, 8,810 clock jumps (1,796 back), 4,268 store outages, 274,560 duplicate deliveries and 89,329 retries after `unavailable`, 1,458 orderly shutdowns, 1,508 lagging subscribers. Checks: 774,877 committed batches through INV-5, INV-6, INV-10 and INV-11, 853,683 acknowledged replies through INV-7, 4,078,053 frames through INV-8 and INV-9, 537,761 replies reproduced by serial replay, 162,043 expiry checks. Seeds 1 to 10,000 (10,000,000 steps) took 35.8 s on 4 threads, every invariant held (trace hash sum `db6701d9994a51ef`; 32,975 failed commits, 33,183 lost replies, 21,620 node crashes, 22,758 takeovers, 3,882,607 batches, 20,458,705 frames).
- **Every check can fail.** Each break below was made by hand, one at a time, never committed, and run as seeds 1 to 400 of 1,000 steps; the first failure is shown, and rerunning its seed alone reproduces it.

  | Break | Reported |
  |---|---|
  | Replies released when the commit starts | INV-7, seed 2, step 1 |
  | A failed commit's callers told it succeeded | failed batch acknowledged, seed 4, step 66 |
  | A batch folded into the published state before it commits | INV-8, seed 1, step 23 |
  | Public snapshot encoded with domain statuses | INV-9, seed 1, step 17 |
  | The feed sends no PRIVATE frame on attach or resync | INV-8, seed 1, step 17 |
  | The store's fencing check removed from commits | INV-11, seed 4, step 697 |
  | The engine skips a `seq` now and then, and the store's gap check is removed | INV-5, seed 1, step 186 |
  | A freed object left out of the batch now and then (engine) | INV-6, seed 4, step 30 |
  | Extended expiries not written (store) | INV-6, seed 1, step 15 |
  | A direct booking accepts an object already booked | INV-10, seed 3, step 37 |
  | A release by token replies with no objects | serial replay, seed 4 |
  | A change-status no-op names `seq` 0 | serial replay (no committed state its client could have seen), seed 4 |
  | A key reused with another request is applied | idempotency (probe), seed 3 |
  | The window never replays, so a duplicate is applied afresh | idempotency (family), seed 2 |
  | Refusals get no idempotency record | INV-7 (keyed reply without a record), seed 2, step 4 |
  | Holds expire, and refuse use, a second after their expiry instant | expiry, seed 4 |
  | A failed load is retried at once | reload storm, seed 4, step 400 |
  | A reload forgets the stored idempotency records | idempotency (probe), seed 4; with the probe and family checks also removed, "ran again under a key already logged", seed 1 |

  One break was not a fault: dropping the per-command expiry and accepting a hold at its expiry instant changes nothing, because the actor's timer arm is polled before the mailbox and always expires a due hold first.
- **Not covered:** the simulation runs one chart of 10 objects and two events; channels, areas and best-available are not built, so INV-4 waits for areas; the client order check has no break of its own (the actor has no code path that reorders its mailbox). The log does not carry a hold's token hash, so INV-6 covers holds by id, expiry, limit and objects; the comparison is the reconciler's own (`Replay::compare`, §3.3), so every committed batch of every run is a reconciler pass on a state the engine reached under faults.

### 3.3 Reconciler

`cavea-server verify` replays `event_log` from the beginning and compares the result with `object_state`, `holds`, `area_alloc` and `events.seq`.

- Used after every load and chaos test.
- Shipped to customers, who can run it after an incident.
- Exit code and a machine-readable report.

**Implemented** (TST-022) as a subcommand of the server binary, not of `cavea`: the server already carries the PostgreSQL driver and the runtime, which would more than double the CLI's dependency tree, and the server image is what a customer has deployed, so after an incident `docker exec <container> cavea-server verify --all` (or `docker run --rm -e CAVEA_DATABASE_URL=… <image> verify …`) runs it with the same settings. Without the subcommand the server starts as before.

```text
cavea-server verify (--workspace KEY --event KEY | --event-id ID | --all) [--json] [--include-booking-data]
```

It reads `CAVEA_DATABASE_URL` only, never prints it, runs no migration and takes no lease. Each event is read in one `REPEATABLE READ READ ONLY` transaction, so its log and its rows come from one snapshot while commits continue, and the actor is never blocked: it takes only the share locks any read takes, which wait for schema changes (a migration, a new log partition) but never for a commit, and the snapshot holds back vacuum while it lasts. The log is read in pages of 10,000 rows in `(seq, at)` order through the primary key, so memory follows the event's state, not its log. The replay is `cavea_engine::Replay`, the same code the model-based tests (§3.1) and the simulation (§3.2) run on every committed batch.

What it checks, per event:

- **Log integrity:** `seq` starts at 1 with no gap and no repeat (INV-5; the primary key allows a repeated `seq` at another `at`); every record decodes and is a legal transition from the state before it (02 §4.3), which is what catches a second booking without a release (INV-10) or a hold past its object limit; every `oid` a record names is in the event's pinned chart version, whose full artefact it decodes as a load does. At the first record the replay refuses, it stops replaying, keeps reading for `seq` gaps, and does not compare state.
- **State (INV-6):** `events.seq` equals the log's last `seq`; every `object_state` row's status, hold, order reference, ticket type, `extra` (byte for byte) and `seq` equal the replay's; rows the log does not explain and rows the log needs but that are missing are reported, as are rows for objects not in the chart; every `holds` row's expiry and object limit equal the replay's, with missing and unexplained holds reported.
- **Not checked, and said so in every report:** a hold's token hash (not in the log, 08 §4.4); `holds.session_sub`, `created_by` and `created_at` (written by the server, not by seat commands); idempotency records (replies are not in the log); hold expiry against time (the log's `at` is the database clock, not the engine clock that expires holds). Areas and channels have no producer yet: any `area_alloc` row and any `object_state.channel` is reported as a difference.

Exit codes: 0 clean, 1 differences found, 2 usage, 3 could not run (database unreachable, unknown event, unreadable chart version, or a database without the schema: it never migrates, so it creates nothing). The default output is a summary, one line per finding; `--json` prints `{"version": 1, "clean", "events": [ … ]}`, each event with `event_id`, `workspace`, `event`, `chart_version`, `clean`, `log_records`, `log_last_seq`, `replayed_to`, `events_seq`, `object_rows`, `hold_rows`, `area_alloc_rows`, `state_compared`, `findings_total`, `findings` (at most 1,000) and `not_checked`. A finding has `check` (`seq`, `transition`, `unreadable_record`, `unknown_object`, `events_seq`, `object`, `hold`, `unreadable_row`, `channel`, `area_alloc`) and, as they apply, `invariant`, `seq`, `oid`, `label`, `hold_id`, `field` (`missing`, `unexpected`, `row_twice`, `booking` for a ticket type or `extra` stored without an order reference, or the column), `stored`, `replayed` (statuses by name: `free`, `held`, `booked`, `blocked`, or a custom code) and `detail`. Order references and `extra` that differ are named but their values are printed only with `--include-booking-data` (12 §4); hold tokens and their hashes never are, and a `detail` that does not decode is reported by the kind of error and its column, never with the decoder's message, which can quote stored values.

**Measured locally** on 2026-11-11 (AMD Ryzen 9 7900X shared with two other builds, PostgreSQL 18 in Docker on loopback; release build): an event on the `stadium-100k` preset whose log (generated with SQL, legal: five rounds of booking and unbooking every object, then a booking of every object) has 1,100,000 rows verifies clean in 1.06 to 1.23 s (about 1,000,000 rows a second) at 65 MB peak RSS; with 3,500,000 rows (17 rounds, the same final state), 4.8 to 5.0 s (about 720,000 rows a second, 2.5 s of it the reconciler's CPU: the rest is PostgreSQL reading log pages its buffer cache does not hold) at the same 65 MB. One `object_state` row's status changed from either is reported as that object and field, in the same time. Time grows with the log; memory with the event's state only.

A log written before hold limits were logged (no released data has one) reads every `CreateHold` as unlimited: each live hold with a limit is reported as a `max_objects` difference until it ends.

Tests (database, `cargo test -p cavea-server … --include-ignored`): an event written by the real engine and `PgStore` from a seeded sequence of every command, expiries included, verifies clean, as does one written through the HTTP API. Each corruption made with SQL is reported exactly: as one finding, a log row deleted (`seq`, the gap; state not compared), duplicated (`seq`, repeated) or two swapped (`transition`); a status flipped (`object`, `status`); a booked object freed without a log row (`object`, `missing`); a second booking logged (`transition`, INV-10); a hold's expiry or limit changed (`hold`, the field); a hold row removed (`hold`, `missing`); a row for a free object (`object`, `unexpected`); `events.seq` moved (`events_seq`); an order reference changed (`object`, `order_ref`, its values withheld unless asked); a held object pointed at another hold (`object`, `hold_id`); a log row with an unknown `op`, one with a negative `seq` (read, not skipped) and one whose `detail` does not decode, its value not shown (`unreadable_record`; state not compared); a row with `seq` 0 (`seq`, out of order); `events.seq` negative (`events_seq`, as stored); a ticket type on a held object (`object`, `booking`); and as exactly these findings, a logged change to an object the chart does not have (`unknown_object`, then `object` `missing`), and the log's last batch lost while the rows it wrote stay (`events_seq`, then its object and its hold `unexpected`). Also every-event mode, the JSON shape and every exit code; and verifying back to back while another task commits 10,000 commands (each run must overlap the commits; 987 runs did with 20,000) reports nothing.

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

Thirty minutes per target, started 2026-11-11 09:31 UTC on the code as it stood just before the commit that added `fuzz/`, each from a corpus already grown by earlier short runs. Coverage is libFuzzer's count of instrumented edges reached (`cov`), at the start of the run and at its end; the corpus is the number of inputs it kept.

| Target | Duration | Executions | Executions/s | Coverage, start → end | Corpus, start → end | Result |
|---|---|---|---|---|---|---|
| `ccf_decode` | 1,801 s | 35,724,651 | 19,836 | 2,701 → 2,759 | 460 → 625 | Ended at its time limit; no crash, panic, timeout, leak or out-of-memory |
| `document_parse` | 1,801 s | 11,572,836 | 6,425 | 13,542 → 16,404 | 1,271 → 5,518 | The same |
| `wire_decode` | 1,801 s | 306,016,899 | 169,914 | 1,302 → 1,327 | 408 → 473 | The same |
| `snapshot_apply` | 1,801 s | 936,538 | 520 | 7,263 → 7,421 | 372 → 1,577 | The same |

The `wire_decode` run started before its check on Pong frames was written, so those 30 minutes did not exercise that check. A second run of 90 s per target, on the final code after rebasing onto the renderer and realtime work (from the grown corpora), also ended clean: `ccf_decode` 2,022,999 executions (coverage 2,759), `document_parse` 546,562 (16,481), `wire_decode` 14,730,333 (1,343), `snapshot_apply` 32,575 (7,855; the renderer engine has grown since the long run). None of this is the 24-hour run of the M1 exit criterion (`ROADMAP.md`), which has not been done. It is one command:

```sh
nix develop .#fuzz -c cargo fuzz run ccf_decode -- -max_total_time=86400 -timeout=10 -print_final_stats=1
```

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

**Payload.** `cargo xtask build-web` measures the brotli size (quality 11) of what a buyer's browser downloads: the production bundle of `@cavea/renderer`'s public entry (`bun build --minify --target browser` with `process.env.NODE_ENV` defined as `"production"`, as a customer's bundler makes it: the SDK and the wasm-bindgen glue, tree-shaken), the WASM after `wasm-opt -Oz`, and the glyph atlas. It runs in CI's `rust` job, which installs bun for it (no `bun install`: the package has no dependencies). Measured 2026-11-11 with the SDK (WEB-001 to WEB-004 and part of WEB-005): bundle 36,361 B raw, 10,646 B brotli (of which the glue, bundled alone, is about 5.3 KB); WASM 83,949 B; atlas 9,462 B; total 104,057 B, against 98,593 B before, when the gate counted the raw glue (7,830 B brotli) instead of a bundle and the WASM was 81,306 B. With the rest of WEB-005 and `caveaServer` (WEB-020 to WEB-025): bundle 50,723 B raw, 15,453 B brotli (the methods 604 B, the adapter, its frame codec and the expiry callbacks 4,203 B); WASM 84,743 B (the methods' camera targets, 794 B); total 109,658 B. With dynamic text, section labels and text objects (REN-026): bundle 52,262 B raw, 15,911 B brotli (+458 B: the text rasteriser and its wiring); WASM 92,410 B (+7,667 B); total 117,783 B (+8,125 B). Of the WASM: the `text` program's shader text 0.8 KB; the label builder about 2.5 KB (section labels with the sold-out line, seat labels from the dynamic atlas, text objects, the build key and upload); the passes, `SetText`, `SetMessage` and the theme's font about 2.2 KB; the dynamic atlas and its packer about 1.0 KB; the bindings and the `textRequest` event's JSON about 0.6 KB (shares of the raw size by function, `twiggy diff`, applied to the brotli total). With row labels, collision and seat and decoration icons (REN-027, REN-028): WASM +3.2 KB, of which the icons' shader text about 1.0 KB (the stroke tables 0.4 KB) and the row labels, the collision pass, the icons' instances and the draw modes the rest. With context restoration and frame statistics (REN-040, REN-044): WASM +2.5 KB (resources made again from the engine's copies, the backend's `restore`, the `contextLost`, `contextRestored` and `frameStats` bindings), bundle +535 B (the context listeners, the 5 s report, the marks and `onMetrics`); total 123,835 B (+6,052 B). Drawing every row label from the dynamic atlas instead of baking the Latin ones (D-41) saved 1,104 B of that.

**Micro-benchmarks.** `cavea bench` (`cargo run --release -p cavea-cli -- bench [name…]`) measures on the `stadium-100k` preset: CCF decode; hit test (10,000 taps near seats at seat sizes of 18 to 64 px with the 12 px touch tolerance, time per test); snapshot apply (100,000 objects) and delta apply (10,000 changes), each from frame bytes through `Command::from_frame`, the engine and the next frame's buffer upload, on a backend that does nothing; seat labels of 4,096 seats (a full render chunk's worth) through `cavea_format::full_label::seat_label`; and compiling each preset. Each benchmark warms up for 0.2 s, then takes 5 to 15 samples of at least 10 ms each (short runs are batched) and reports the median and the minimum time per run, with the budget beside it where one has a hard number: decode 5 ms per 100,000 seats (`03-chart-format.md` §4.4), hit test 20 µs (`04-renderer.md` §15), delta apply 2 ms (`01-architecture.md` §5), label build 1 ms (`04-renderer.md` §14, "well under a millisecond"). A missed budget exits 4. `--json` prints the same as JSON. It refuses to run in a debug build.

The gate compares a machine with itself: `--save <file>` writes the medians, and `--baseline <file>` compares a later run and exits 4 when any benchmark's median is more than 10% slower. A builder runs it with `--save` on `main` and `--baseline` on the branch before filing a change to the measured code; a flagged result is re-run before it is believed (the median of 5 to 15 samples absorbs most noise, but not another process taking the core). No baseline is committed and CI runs none: a number from one machine says nothing about another.

Measured locally, 2026-11-11, AMD Ryzen 9 7900X, Linux, release build, with no fuzzer running but other agents' work on the machine (load average about 5 on 24 threads); not reference-device results:

| Benchmark | Median | Minimum | Budget |
|---|---|---|---|
| decode (100,000 seats, 864,477 B) | 0.533 ms | 0.532 ms | 5 ms |
| hit test (per tap, seats at 18 to 64 px) | 0.296 µs | 0.293 µs | 20 µs |
| snapshot apply (100,000 objects, 12,509 B frame) | 0.168 ms | 0.168 ms | none |
| delta apply (10,000 changes) | 0.187 ms | 0.186 ms | 2 ms |
| label build (4,096 seats) | 0.160 ms | 0.160 ms | 1 ms |
| compile `theatre` (2,000 seats) | 1.640 ms | 1.604 ms | none |
| compile `arena` (20,000 seats) | 14.708 ms | 14.525 ms | none |
| compile `stadium-60k` | 45.671 ms | 45.454 ms | none |
| compile `stadium-100k` | 81.100 ms | 79.603 ms | none |
| compile `stress-250k` | 200.248 ms | 197.074 ms | none |
| compile `stress-500k` | 378.726 ms | 362.338 ms | none |

A second run straight after, with the first as its `--baseline`, was within 6% on every benchmark (compiles from 5.6% faster to 1.4% slower, the rest within 1.5%) and passed the gate.

**Payload gate.** `cargo xtask build-web` measures the renderer payload, the brotli size (quality 11) of the production JavaScript glue plus the WASM after `wasm-opt -Oz` plus the glyph atlas (`packages/renderer/assets/glyphs.png`, a PNG, so about its own size), and compares it with `xtask/payload-baseline.json`, which records the last accepted size and the `wasm-opt` version it was measured with. The build fails when the payload is over the hard cap of 200,000 B (`01-architecture.md` §5), or more than 3% over the baseline. It refuses to compare a build made without `wasm-opt` (about a fifth larger) or with another `wasm-opt` version than the baseline's: either is an error, not a pass. CI installs binaryen 132, the version `flake.nix` provides; upgrading binaryen changes the flake, CI and the baseline together. The `--demo` build is never measured.

The justification is the baseline file itself, not a label: a pull request that grows the payload by more than 3% on purpose runs `cargo xtask build-web --update-baseline` and commits the file, so the new size is in its diff and its description says why. Growth under 3% passes without an update, and so does a decrease; growth then adds up against the same baseline until it crosses 3%. Refresh the baseline in any pull request that moves the payload more than 3% either way (the build says so when it shrinks), and when the orchestrator asks after another pull request has changed it.

## 7. Server load and chaos

### 7.1 Load generator

`cavea-loadgen`, written in Rust, reusing the wire codecs. It simulates buyers: connect, receive the snapshot, pick seats (random, clustered or best-available), hold, optionally release, and a "backend" that books a share of holds.

**Implemented** (TST-023) in `crates/cavea-loadgen`, a binary that drives a running server over its public and secret-key HTTP API and its WebSocket, as real clients do. Its dependencies are all already in the workspace's tree (tokio, `tokio-tungstenite` at axum's version, `futures-util`, serde, `cavea-format`); HTTP is a small keep-alive client of its own over tokio's `TcpStream`. It speaks no TLS and refuses `https` URLs: it runs next to the server, on loopback or a private network.

```text
cavea-loadgen --url http://HOST:PORT (--chart DOC.json | --event KEY) [--profile l1|l2] [options]
```

- **Setup.** With `--chart` (a document from `cavea gen`) and `CAVEA_ADMIN_KEY`, it creates a workspace `lg-<seed>-<ms>`, a secret and a publishable key, publishes the chart and creates event `loadgen` (`--hold-ttl`, default 60 s; `--max-objects`, default 10), so a run starts from an empty database and `cavea-server verify --workspace lg-… --event loadgen` follows it. With `--event`, it uses `CAVEA_SECRET_KEY` and `CAVEA_PUBLISHABLE_KEY`. Keys are read from the environment only and are never printed or reported. It then reads the public event, decodes the compiled chart for its rows' segments, and maps every `oid` to its `idx` from the objects query.
- **Subscribers** fetch the snapshot (`…/state?key=`), open the WebSocket, SUBSCRIBE from the snapshot's `seq`, and apply every frame to a view of their own (one bit per object) through `cavea_format::wire`: HELLO and SNAPSHOT checked for the object count, a DELTA only from the view's own `seq`, nothing between RESYNC and its SNAPSHOT, no SNAPSHOT behind the `seq` the view had reached (a published state, so a committed one, would have been lost), PING answered, CLOSE counted. Any frame that breaks this is a protocol violation. A dropped connection (an error, 60 s of silence, CLOSE 3 or 6 or an unknown reason) is reconnected as the SDK does (05 §4.1): after `min(30 s, 500 ms × 2^(n−1)) × U(0.5, 1)` for the n-th consecutive failure, a HELLO resetting the count, SUBSCRIBE from the view's `seq` (the server sends the DELTAs since or a SNAPSHOT); CLOSE 1, 4 and 5 and a protocol violation end it. The first `--probes` (8) measure delta delivery: the time from a buyer's hold reply to the probe applying the DELTA that covers its `seq`.
- **Buyers** create a token (`POST /v1/public/hold-tokens`), pick 2 to 4 seats their view shows available, adjacent in one segment of a row (`--pick clustered`, the default) or anywhere (`random`), and hold them; after `object_not_free` they pick again, twice at most. Then by share: release after up to a second's thought (`--release-share`), hand the hold to the backend (`--book-share`), or let it expire. In a stampede every buyer is also a subscriber and picks from its own view; in a sustained run buyers arrive without subscribing and pick from a random subscriber's view. A token the server does not know (`404`) ends the buyer's session; it is a `lost_ack` if its creation was acknowledged and it is certainly unexpired.
- **Backend** (32 connections, secret key): books with the hold token, an order reference and an `Idempotency-Key`, sending `--retry-book-share` (0.1) of bookings twice, where the repeat must get the first reply (`200`, the same `seq` and objects; anything else is `idempotency`); cancels each booking `--unbook-after-ms` after it (release without a token, from a pool of keep-alive connections); blocks a free seat and unblocks it 200 ms later, `--blocks-per-sec` times a second. Every backend command carries an `Idempotency-Key`, so a retry of one that was applied gets its original reply.
- **Outcomes and retries.** Every request ends as a reply; as certainly not applied (no connection could be opened, `503 overloaded`); or with an unknown outcome (the connection broke after the request was written, no reply within 10 s, `503 unavailable`, which a failed or timed-out commit also gives, 08 §4.6). The last two are outage errors, counted by kind, and the request is tried again after the SDK's backoff: a buyer's at most six times within 20 s of the first attempt, then the buyer gives up; the backend's for up to 60 s. A request given up after an unknown attempt is recorded as unknown. A buyer's hold that succeeds after an unknown attempt may be a no-op naming the `seq` it observed, so it is marked retried (its `seq` is not required to be its own, and its ownership starts at the first attempt); a release that then frees nothing means the earlier attempt was applied, and is recorded as an unknown release. An outage is reported from the first failed attempt to the next hold that succeeds, and each subscriber disconnection from the drop until every subscriber is connected again.
- **Shapes.** `l1` is a stampede: `--subscribers` 50,000 watchers connect first (at 2,000 a second), then `--buyers` 5,000 holds spread evenly over `--ramp-ms` 3,000. `l2` is sustained: `--rate` 2,000 buyer arrivals a second for `--duration-s` 600, with 100 watchers. Every flag scales them down. Arrivals are open-loop: started on a clock, never waiting for earlier ones; at most `--max-in-flight` (4,096) run at once, and an arrival that finds the bound reached is skipped and counted, so offered and achieved rates are both reported. A sustained run must not fill the event: seats held at once are about `rate × seats per hold (3) × (release share × 0.5 s + book share × unbook delay + expire share × hold lifetime)`, so `l2` defaults to releasing 89%, booking 10% (cancelled after 5 s) and letting 1% expire: about 10,000 seats at 2,000 arrivals a second with a 60 s lifetime, half a 20,000-seat event.
- **Latency.** Each request is timed from when it is sent; the arrival's first request (token creation) from its scheduled start, so a generator that starts an arrival late charges the server for it. A hold is sent as soon as the token reply arrives, so its time from sending is the server's; `hold_from_due` times the whole arrival from its scheduled start, and `start_lag` how late the generator started arrivals (its p99 is in the report, which flags a generator that fell behind: more than 5 ms). The budget's "hold request" is compared with `hold` (from sending). The generator's and the server's CPU (from `/proc`) are reported over the load and per interval.
- **Client addresses.** Every simulated client must be its own address, or the per-address limits (120 holds a minute, 200 realtime connections, 08 §12) measure the limiter. `--source loopback` (the default for a loopback server on Linux) binds each subscriber and buyer to its own address from 127.1.0.1 upwards (all of 127.0.0.0/8 is local on Linux, and each address has its own ephemeral ports, so tens of thousands of connections to one port do not run out of ports); `--addresses` defaults to one per subscriber and buyer, and in a sustained run one per arrival over 30 s, so no address passes its budget. The server runs with its default limits. `--source forwarded` sends `X-Forwarded-For` from 10.0.0.0/8 instead, for a server elsewhere that lists the generator in `CAVEA_TRUSTED_PROXIES`; `--source single` uses one address and needs `CAVEA_RATE_LIMIT_HOLDS=0`, `CAVEA_RATE_LIMIT_READS=0` and a high `CAVEA_RT_MAX_PER_IP`. L1's 55,000 connections fit the default `CAVEA_RT_MAX_CONNECTIONS` (60,000). Each connection is a file descriptor on both sides: the generator checks its own open-file limit before it starts (about subscribers + 2 × buyers + max in flight) and stops with the `ulimit -n` to set; the server needs the same. While it still has one thread the generator opens and closes that many descriptors, so its descriptor table is already that size: Linux grows the table by doubling, and in a process of several threads each doubling waits for an RCU grace period (10 to 21 ms measured on this kernel, none with one thread) during which no thread can open a socket, which would be charged to the server.
- **What it checks.** After the run, every acknowledgement (token creation, hold, release, booking, cancellation, block, unblock), in `seq` order: two parties told they hold one seat while the first's hold was certainly live (`double_hold`; certain means its token's expiry, bounded on the server's clock from token creations, whose `expires_at` minus the lifetime is a server time between the request and its reply, is more than 5 s away); a hold of a booked or blocked seat; a release, booking or cancellation of a seat its party did not have (`transition`); two acknowledgements of one `seq`. Every `object_not_free` refusal must name only requested seats, with reason `not_free`, and be explained by another party that may have had the seat between the request and its reply (an acknowledged ownership, a hold, booking or block whose outcome is unknown, or a seat unavailable before the run): otherwise `wrong_refusal`. `hold_limit_exceeded` only for a request over the limit; a reply's objects equal to what was asked; any other status is reported. These checks accuse only with replies the generator received: an unknown outcome only ever excuses (a release, cancellation or unblock sent before another party's acknowledgement may have freed the seat). At the end, after `--quiet-ms`, every live subscriber's view must equal a fresh snapshot once it reaches that snapshot's `seq` (retried for up to 60 s while later commits move it or subscribers reconnect); a view never compared (`unverified`) fails the run, as one that differs does.
- **Audit.** Then the committed state is read: every object not free (the objects query, secret key) and the hold of every token that may still hold something or was refused a hold (`GET /v1/hold-tokens/{token}`, 16 at a time), and compared with what the generator was told: an acknowledged booking, block or certainly live hold still in effect must be there with its order reference or token (`lost_ack`), unless a later unknown cancellation, unblock, release or booking explains it; a seat booked under a booking the generator was told failed, or held by a token whose holds were all refused, is `failed_took_effect`; a seat held, booked or blocked by nothing acknowledged or unknown is `unexplained` (or `lost_ack` if its release, cancellation or unblock was acknowledged); a seat held more than 5 s past its expiry is `expiry_late`. A hold listed and then expired before its token was read is not an accusation. Each unknown outcome is reported as applied, not applied, or undecidable (its token has expired since, or a later command changed the seat). With `--acks FILE` every acknowledgement whose `seq` is its own is written as `seq,op,oids` (CSV), to be joined with `event_log`: each must be there at its `seq` with its `op` and objects. What the generator cannot see: anything between commit and reply that no reply reveals, the exact instant of an expiry (hence the 5 s margin), the order of a retried no-op, and the server's state beyond the public view, its replies and the committed rows: that is the reconciler's (§3.3), which every run should be followed by.
- **Report.** A summary on standard output and, with `--json FILE`, `{version, profile, settings, wall_s, load, requests, ops, delta_delivery, connections, checks, budgets, outages, subscriber_gaps, cpu, audit, timeseries, exit}`: per operation `ok`, `errors` by code and p50, p95, p99, max (from a fixed 1,920-bucket histogram, at most 3.2% error); requests retried after an outage and given up, and unknown outcomes; connections opened, failed, given up, reconnected, resyncs and closes; violations (the first 100, and the total); convergence counts; the 01 §5 budgets beside the measured figures (hold p99 15 ms, snapshot p99 3 ms, delta p95 250 ms); outages and subscriber gaps with their Unix times; CPU; the audit's counts and the unknowns' resolution; every `--interval-ms` the holds and their p50, p99 and max in that interval, refusals, errors, work in flight, open subscribers, the generator's RSS and cores and, with `--server-pid` on the same machine, the server's. Exit 0 all correct and within budget, 1 a correctness check failed, 2 usage, 3 could not run, 4 correct but a budget missed.
- **Tests.** The histogram against a uniform distribution; the open-loop schedule under paused time (a server taking a second per request does not lower the offered rate; the in-flight bound skips, never delays) and with the generator's only thread blocked (late starts keep their scheduled `due`, so the stall is charged); the backoff against the SDK's formula; the view (a snapshot plus random coalesced deltas equals the statuses applied directly; a gap, a DELTA during RESYNC and a SNAPSHOT behind the view are refused; a reconnection resumes from the view's `seq`); the picker; the checker on hand-made histories, including what it must not flag (a seat released, expired, or cancelled and then held by another; a release, cancellation or unblock with an unknown outcome before another party's hold; a retried success sharing a `seq`); the audit (each acknowledged kind missing, a refused hold and a refused booking in effect, an acknowledged release, cancellation and unblock not in effect, an expiry long past, the read race, and the resolution of unknown holds and cancellations); arguments and the report's shape and exit codes. With `CAVEA_TEST_DATABASE_URL`, `tests/live.rs` starts the workspace's `cavea-server` on a database of its own, runs a stampede, a sustained run with blocks, and a sustained run during which the server is killed (SIGKILL) and started again on the same port, and requires no violation, converged views (none unverified after the kill, with reconnections), and `cavea-server verify --all` clean.
- **Every check can fail.** Each break below was made by hand in the server, one at a time, never committed, and run against the generator (a sustained run of 300 arrivals a second for 10 s unless said otherwise):

  | Break | Reported by the generator | Reconciler |
  |---|---|---|
  | A hold of free seats refused when `seq` is a multiple of 29 | 348 violations, the first 100 (all the report lists) `wrong_refusal` | clean (refusals are not logged) |
  | A held seat may be held again | 21 `double_hold`, 12 `transition`, 2 `lost_ack` | refused the log |
  | `Idempotency-Key` ignored | 436 violations, the first 100 `idempotency` (the repeat got `404`, the first `200`) | not run separately (one database with the break above) |
  | A successful public hold answered `429` now and then | 293 violations; of the first 100, 99 `failed_took_effect` and 1 `wrong_refusal` it caused | clean |
  | Replies released when the commit starts, during PostgreSQL faults (§7.4) | 23 `seq`, 23 `double_hold`, 4 `lost_ack`, 8 `release_reply`; the log comparison found 19 acknowledgements whose `seq` holds another command | clean |

  The last break was also run under three `kill -9`s of the server, and nothing was lost: all 208,699 acknowledgements were in the log. PostgreSQL completes a statement it has received even after its client dies, so killing the server does not abort the commit in flight; only a fault of the database's own (§7.4) makes an early reply a lost one. The checks not shown with a server break (`expiry_late`, a SNAPSHOT behind the view, `unexplained` for a booking or a block) are covered by the unit tests above.

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

### 7.4 Results, measured locally

Not reference-hardware results (§7.2): measured on 2026-11-11 on a desktop shared with two other builders. **Conditions for every run:** AMD Ryzen 9 7900X (12 cores, 24 threads, two CCDs), about 30 GB of memory, Linux 6.17; release builds; the server pinned to CPUs 6, 7, 18 and 19 (two cores and their SMT siblings, like a 4-vCPU cloud machine), the generator to CPUs 0 to 3 and 12 to 15 (the other CCD); PostgreSQL 18.6 in Docker on the same machine, unpinned, on the desktop's SSD, `synchronous_commit = on`, `fsync = on`, `wal_sync_method = fdatasync` (read from the private instance; the shared one runs the same image and defaults); loopback network; the server with its default settings (`CAVEA_LOG=info`, JSON to a file, `CAVEA_DB_POOL` 16, default rate and connection limits) and loopback client addresses; one event on `arena` (20,000 seats), 60 s holds, `max_objects` 10. Load and `kill -9` runs used a database of their own on the shared PostgreSQL; the PostgreSQL experiments a private instance (its own compose project, port and volume, removed afterwards). Load averages are of the whole machine, the run included (it adds about 5).

**What these runs do not show:** the reference machine; a database on its own host and storage (a desktop SSD shared with compilers, in Docker); a real network (on loopback the sender's `write` also runs the receiver's TCP stack, so the server's cores pay for both ends of every frame); more than one event; browsers (the generator's clients reuse a keep-alive connection per buyer).

**L1** (`--profile l1`: 50,000 watchers connecting at 2,000 a second, then 5,000 buyers, each also subscribed, holding within 3 s; 55,000 connections), run twice on one server process, load average 3.7 at the start and 3.4 at the end:

| | First run (cold process) | Second run (same process) | Budget or criterion |
|---|---|---|---|
| Hold p50 / p99 | 901 ms / 1,311 ms | 918 ms / 1,245 ms | p99 ≤ 15 ms: **missed** |
| Snapshot p50 / p99 (55,000 requests) | 0.42 ms / 2.43 ms | 0.42 ms / 1.18 ms | p99 ≤ 3 ms: met |
| Delta delivery p50 / p95 | 258 ms / 401 ms | 229 ms / 344 ms | p95 ≤ 250 ms: **missed** |
| Connections | 55,000 opened, none failed or dropped | the same | ≥ 50,000 per node |
| Refusals (`object_not_free`) | 2,767, every one explained | 2,638, every one explained | correct reasons: met |
| Double holds, other violations, audit | none | none | none: met |
| Views equal to the final snapshot | 55,000 of 55,000 | 55,000 of 55,000 | |
| Acknowledgements in the log | 12,887 of 12,887 | 12,936 of 12,936 | |
| Reconciler | clean | clean | clean: met |
| Server RSS | 572 MB with the 55,000 connections open, 1,170 MB after the buyers' 5 s | about the same | 8 GB |

L1 does not pass: hold latency is 80 times its budget. The cause is CPU: in the buyers' 5 s the server uses 3.4 to 3.8 of its 4 cores (the generator 2 to 5 of its 8, never short), and requests queue behind the fan-out of deltas. With more than 20,000 subscribers a publication goes out every 275 ms (08 §8.3) to 55,000 sockets, about 200,000 frames a second of about 3 KB each during the stampede, each its own write, on the same 4 threads that serve requests. Holding the buyers constant and varying the subscribers shows it (the same profile, each a fresh process):

| Subscribers | Server CPUs | Hold p99 | Delta p95 |
|---|---|---|---|
| 5,000 (the buyers only) | 4 | 21.5 ms | 92 ms |
| 25,000 | 4 | 491 ms | 82 ms |
| 55,000 | 4 | 1,245 to 1,311 ms | 344 to 401 ms |
| 55,000 | 8 | 217 ms (the generator, now short of CPU itself, started arrivals up to 233 ms late) | 451 ms |

Hold latency follows the number of subscribers, not the number of holds. Loopback inflates the cost per frame (above), so a real network would do better; but the budget's 15 ms at 50,000 subscribers on 4 vCPUs needs the fan-out to stop competing with requests (a runtime or threads of its own, or cheaper writes per frame), which is a change of design, not of this slice.

**L2** (`--profile l2`: 2,000 arrivals a second, 100 watchers, 5 blocks a second): run for the full 600 s, load average 1.4 at the start, up to 11 during it, 1.6 at the end. Correct throughout: 1,083,588 holds, 964,180 releases, 119,551 bookings, 108,535 cancellations; no violation; all 100 views converged; the event never filled (at most about 2,800 seats not free); all 3,359,700 acknowledgements in the log (4,340,769 records); reconciler clean. Server RSS rose to 141 MB in the first 2 minutes (about 120,000 live tokens at 60 s each), stepped to 265 MB in the backlog after the first stall below, and stayed at 265 MB from 210 s to 600 s: **memory flat**. Commits: 187,270 batches, 23 commands on average (p50 21, p99 74, at most 1,201), 260 to 300 a second (from `event_log` rows sharing a statement's `at`). Delta delivery p95 96 ms (budget 250 ms: met).

But from 120 s on, about every 60 s, new connections stopped for about 10 s: arrivals backed up to the in-flight bound (104,876 of 1,200,000 skipped), token creations timed out connecting, and the server sat at 0.2 cores. The cause is the machine: Docker loads connection tracking, whose table holds 262,144 entries, and a closed connection stays in it for 120 s, so 2,000 new connections a second (one per arrival, as browsers make) plus the backend's fill it at 120 s; then new SYNs are dropped until entries expire. A server host with Docker's defaults has the same ceiling, about 2,180 new connections a second sustained: raise `nf_conntrack_max` or exempt the port from tracking. With the stalls, the whole run's hold p99 was 254 ms. A second run of 150 s with cancellations on pooled connections stalled twice (at 133 and 143 s) and measured hold p50 7.0 ms, p95 13.6 ms, p99 30.7 ms (per-second p99 median 11.8 ms, above 15 ms in 29% of seconds); a 15 s run, 11.0 ms. Against a private PostgreSQL with no other clients (100 s, no stall): p50 6.7 ms, p95 11.0 ms, **p99 23.0 ms**, the server at 1.4 of its 4 cores. So the hold p99 budget (15 ms at 2,000 holds a second) is **missed** here, and not for lack of CPU: the tail follows commits, which wait on the WAL's `fdatasync` on a desktop SSD that the neighbours' builds share (batches of over 1,000 commands mean a commit that took 150 ms or more). It needs a measurement on a database host of its own before it says anything about the server. The snapshot route's p99 (only 100 snapshots, at the start) was 12 to 17 ms, for the reason below.

**Snapshot p99 at a connection burst.** At 2,000 new connections a second, a fresh server process answered snapshots at p99 25.6 ms; a second burst on the same process at 3.3 ms; at 500 a second 1.8 ms. The server's own durations for the route were at most 4.7 ms (p99): the rest is before the handler. Linux grows a process's descriptor table by doubling, and in a process of several threads each doubling waits for an RCU grace period, measured here at 10 to 21 ms per doubling (64, 128, … 16,384) and nothing in a single-threaded process; no thread can accept a connection meanwhile. Going from 30 descriptors to 6,000 costs seven such stalls. At L1's scale it amortises (p99 2.43 ms). A server that wants a clean first on-sale after a restart can open, while it is still single-threaded at startup, as many descriptors as it expects to hold and close them again; the generator does exactly that (§7.1). The warm p99 at 2,000 new connections a second, 3.3 to 3.5 ms, is over the 3 ms budget by the cost of accepting a connection on 4 busy cores.

**`kill -9` during sustained load** (1,500 arrivals a second, about 4,500 commands a second, so a commit is always in flight; 6 kills at random moments in 100 s, each followed by a restart after 0.2 to 2 s; load average 4.0 to 7.6): no violation; reconciler clean (566,786 records); all 461,314 acknowledgements in the log at their `seq` with their `op` and objects; all 100 views converged. 15,757 requests met the outage and were answered on a retry; 15 releases were left unknown, and were undecidable by the audit (their tokens had expired). A hold is acknowledged after its commit, and PostgreSQL finishes a statement whose client has died, so even the deliberately broken early-reply server lost nothing to `kill -9` (§7.1). Recovery, from the restarted server answering:

| Kill | Server down (kill to answering) | First successful hold after | Every subscriber back after |
|---|---|---|---|
| 1 | 1.71 s | 0.00 s | 1.50 s |
| 2 | 1.02 s | 0.04 s | 1.72 s |
| 3 | 0.63 s | 0.09 s | 0.80 s |
| 4 | 1.21 s | 0.06 s | 1.79 s |
| 5 | 1.51 s | 0.11 s | 1.86 s |
| 6 | 0.89 s | 0.08 s | 1.34 s |

Subscribers come back within the SDK's backoff (05 §4.1). Holds live at a kill expire on schedule after the restart: 2,400 holds live at a kill, the server restarted with no client at all, it preloaded the event in 21 ms (08 §4.7), and every hold expired by the log 2 ms after its instant on average, 6 ms at most; none was left; reconciler clean.

**PostgreSQL restarts during sustained load** (private instance; 1,500 arrivals a second; 6 faults at random moments in 110 s, alternately `docker restart` and `docker kill` then `docker start` after 0.5 to 3 s; load average 1.1 to 2.5): the server was never restarted and recovered by itself every time; every error during the outages was `503 unavailable` (7,537 attempts; 6,582 requests met the outage and were answered on a retry); no violation; reconciler clean (611,573 records); all 488,778 acknowledgements in the log; no outcome left unknown; all 100 views converged. The actor stopped at each fault (a failed commit or heartbeat) and reloaded on the next request; 5 loads failed while the database was coming back and were retried after the 1 s hold-off (08 §4.6).

| Fault | Database down (fault to accepting connections) | Outage seen by buyers | First successful hold after the database was back | Every subscriber back after |
|---|---|---|---|---|
| restart | 1.00 s | 1.62 s | 0.63 s | 6.83 s |
| kill | 2.46 s | 3.69 s | 1.25 s | 0.93 s |
| restart | 0.67 s | 1.15 s | 0.50 s | 0.19 s |
| kill | 2.52 s | 3.73 s | 1.22 s | 0.87 s |
| restart | 0.83 s | 1.24 s | 0.43 s | 0.03 s |
| kill | 2.51 s | 2.90 s | 0.41 s | 0.84 s |

**Defects found.**

1. *Fixed:* during a PostgreSQL restart the server answered `500 internal_error`, which clients do not retry, where 08 §16 promises `503`: a connection made while the database was down got an answer from Docker's port proxy that was not PostgreSQL's (`unexpected response from SSLRequest`, a protocol error), and PostgreSQL ended connections as it stopped (`57P01`). In one run: 1,463 token creations, 572 releases and 8 holds. A database error now maps to `503 unavailable` for a broken or refused connection (I/O, TLS, protocol, a crashed worker, SQLSTATE class 08, `57P01` to `57P03`, `53300`) as well as an exhausted pool; the regression test is `http::error::tests::a_database_going_down_is_unavailable`. The run above is after the fix.
2. *Not fixed* (it belongs to `cavea-telemetry`): **a server whose log output fails stops serving.** When `tracing-subscriber` cannot write an event, it reports it with `eprintln!` (its `log_internal_errors`, on by default), and `eprintln!` panics when standard error cannot be written either. So with its log file's disk full, or a closed pipe, every task that logs panics: each request's connection is dropped without a reply when its record is written, the event's actor panics on its next log line, and the event is never reloaded (its load logs too). Found when the machine's `/tmp` filled during a PostgreSQL experiment: for 8 minutes the server answered almost nothing, and shutdown missed its deadline; nothing acknowledged was lost (reconciler clean, every acknowledgement in the log). Reproduced with the output sent to `/dev/full`: at the default level the server dies at its first log line; with `CAVEA_LOG=error` it serves. A fix is to turn `log_internal_errors` off and give the layer a writer that drops what it cannot write.

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
