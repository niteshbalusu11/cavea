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
| `http_api` | Requests generated from the OpenAPI document against the server's router |

- Seed corpora are kept in the repository.
- Nightly: 30 minutes per target. Any crash, panic or hang is a release blocker.
- The code under fuzzing is compiled with overflow checks and debug assertions on. libFuzzer runs native code only, so the targets fuzz the same crates the WASM build ships, natively.

**What exists.** The crate `fuzz/` holds five `cargo-fuzz` (libFuzzer) targets. It is its own cargo workspace with its own lockfile and target directory, so the main workspace's build, lint and CI never see it; it needs a nightly compiler, which the second dev shell `nix develop .#fuzz` provides with `cargo-fuzz` (the default shell keeps the pinned stable toolchain). Its release profile turns on overflow checks and debug assertions; `cargo fuzz` adds AddressSanitizer.

| Target | Property checked on every input |
|---|---|
| `ccf_decode` | Bytes into `decode` as both artefacts. An accepted chart passes `Chart::check`, re-encodes, re-decodes to the same chart, and re-encodes to the same bytes again. A file accepted as the full artefact is accepted as the public one with the same chart less the full-only data |
| `document_parse` | Bytes into `ChartDocument::from_json`; a parsed document saves and loads back unchanged, then compiles. A refusal lists errors only; a compiled chart carries warnings only, both artefacts decode to the same chart, and their stored content hashes are their BLAKE3 hashes |
| `wire_decode` | Bytes into `decode_server` (public and manager views) and `decode_client`. Accepted frames: codes within the view, every `idx` strictly increasing and below the object count, a receiver with fewer objects refused, a snapshot's runs covering exactly its objects; re-encoding the decoded values and decoding again gives the same values |
| `snapshot_apply` | A fixed 102-seat, two-floor chart and a fuzzer-chosen sequence of chart loads, server frames (through `Command::from_frame`), selections, hold results, session modes and frames, into the renderer engine on a backend that asserts every buffer and texture write and every drawn instance range lies inside the resource it created |
| `http_api` | A sequence of up to 12 requests, each an operation of the OpenAPI document with a credential, parameters, headers and a body generated from it, into the server's router in process over a database of the run's own (below). No panic anywhere in the process and no `5xx`; every answer's status is one the document lists for its operation and its body validates against the document's schema, and a JSON body answered `2xx` validates against the request schema (the router tests' own check, `http::openapi::check`); a `2xx` only with a credential the operation accepts; nothing of the other workspace (its id, key ids, keys, hold tokens, names) in an answer to a workspace's credential unless the request carried it |

Every target is bounded in time by `-timeout=10` (a hang is a finding) and in memory by libFuzzer's default 2,048 MB limit. Seeds are `fuzz/corpus/<target>/seed-*`, 31 files and 54 KB in all: the compiler's stored schema samples, the fixed chart and a 240-seat synthetic stadium as documents and as both artefacts, refusals the parser must give cleanly, one valid frame of each type, and four engine scenarios. `cargo run --example seeds` in the fuzz shell writes them again after a format change. libFuzzer adds the inputs it finds to the same directory under hash names, which are gitignored, as are `fuzz/artifacts/` (crashes) and `fuzz/target/`.

**The HTTP target.** `http_api` (`fuzz/fuzz_targets/http_api.rs`, D-78) builds the server's router from outside the crate as `cavea-server`'s `main` does and sends requests through it with `tower::ServiceExt::oneshot`: no socket. The handlers query PostgreSQL directly (about 80 statements under `http/` alone), so there is no in-memory server to fuzz; the run creates a database of its own on the server `CAVEA_FUZZ_DATABASE_URL` names (`cavea_fuzz_<pid>_<n>`, migrated, `synchronous_commit` off), drops it at exit, and drops at start any such database whose process is gone (a crash leaves its database). The server crate's one seam is the feature `fuzzing`, which makes the router tests' OpenAPI check (`http::openapi::check`) public; no build of the server enables it. Each input runs in a world of its own, so a finding reproduces from its input file: two fresh workspaces made through the API, with the same chart and event keys and keys of their own, each with a publishable key, a revoked secret key, a published chart, a draft chart, an archived chart, events (one requiring session tokens, one archived) and a hold token holding a seat; worlds are made eight at a time, so their database round trips overlap. An input is records, each one request (`fuzz/src/http.rs`): a length byte, an operation (by index into the document's operations), then choices read in order where a missing byte is 0 and 0 is the ordinary choice, so a seed is a list of operations and the generator spends its effort past authentication and validation. The credential is the operation's right one most of the time, else the other workspace's, none, malformed, unknown, revoked or the wrong kind; path, query and header values come mostly from the world (its keys and labels, its hold tokens, the draft's etag, session tokens signed with its keys, valid, expired, for another event or by a revoked key, three idempotency keys so that replays and mismatches happen) and sometimes from the input's bytes; a body is generated from the operation's schema, with fields named like the world's things taken from it, text often at the bounds of the 1-to-128-character rule, and now and then a field left out, an unknown field, a value of the wrong kind or raw bytes; a method or path is changed now and then. Left out: the four console routes that hash a password (sign-in, setup, password change, accepting an invitation: argon2 on a blocking thread would dominate every input; the console's own tests cover them), and console sessions as a credential for the same reason. The engine runs on the system clock: expiry is also decided in SQL on the database's clock, which a frozen engine clock would contradict. Build it without AddressSanitizer (`-s none`): the code is safe Rust, and the sanitizer halves a throughput the database already bounds. Event actors stay loaded 10 to 20 minutes after their last command (08 §4.7) and the target makes about 18 worlds a second, so its memory grows for the first 20 minutes of a run: give long runs `-rss_limit_mb=4096`.

```sh
CAVEA_FUZZ_DATABASE_URL=postgres://cavea:cavea@127.0.0.1:5432/cavea \
  nix develop .#fuzz -c cargo fuzz run -s none http_api -- -max_total_time=n -timeout=10 -max_len=512 -len_control=0 -rss_limit_mb=4096
```

It can fail: three bugs were planted in turn, each found and then removed (5-minute budget each, from the seeds, one core): an `unwrap` on an empty `order_ref` in the booking handler, 239 s (3,697 executions; the first two attempts found the two real findings below first, and a third did not reach it in 300 s until the generator was made to produce text at the API's bounds); the key revocation route answering `202`, which the document does not list, on the first input (world seeding revokes a key); the key lookup accepting revoked keys, 30 s (548 executions: `POST /v1/keys` succeeded with a revoked key).

The timed run, on the final code of the target (local, AMD Ryzen 9 7900X shared with other builders, one core, the shared PostgreSQL 18 in Docker), started 2026-11-11 from the seeds alone:

| Target | Duration | Executions | Executions/s | Coverage, start → end | Corpus, start → end | Result |
|---|---|---|---|---|---|---|
| `http_api` | 1,802 s | 26,657 | 14 | 16,844 → 22,177 | 62 → 2,561 | Ended at its time limit; no crash, panic, `5xx`, document disagreement, credential or isolation failure, timeout or out-of-memory (peak RSS 1,210 MB) |

Each execution is up to 12 requests after the world's 34 (two workspaces), so the run sent roughly 1.2 million requests. A clean 5-minute run before the plants: 5,564 executions, coverage 20,048.

Findings (release blockers by the rule above):

- **Fixed:** a path or query holding U+0000 answered `500` (`GET /v1/events/q%06%00q%00/objects`): the key reached PostgreSQL, whose text cannot hold it. The server's `Path` and `Query` extractors now answer `404` and `400` (`reason: "query"`), 08 §6.4. Its input is the seed `seed-found-nul-in-path`.
- **Known, not fixed:** the server takes a JSON array for a request object, its fields by position (serde's derived `Deserialize` accepts a sequence for a struct): `PATCH /v1/events/{event}` with `[]` answered `200`, and `["e1"]` reads as `{"event": "e1"}` on hold-token creation. The document allows only objects. Refusing it needs every request type, nested ones too, to accept only a map; it is the server's to fix. Its input is `fuzz/known/http_api/array-for-object`, outside the corpus so that runs do not stop on it; while `KNOWN_ARRAY_FOR_OBJECT` stands in the target, a body whose top level is an array is not checked against the request schema and the generator puts no array where the schema has an object.

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

**Golden images** (`cargo xtask golden [--update] [--runs n] [case…]`, D-66). Builds the release WASM (without the payload gate) and the CLI, compiles the charts `crates/cavea-render/golden/cases.json` names (a preset, optionally with another seat shape or relabelled sections, rows, seats and text objects), renders every case through the headless mode (04 §26) in the flake's Chromium on SwiftShader with only the pinned DejaVu Sans to draw text, and compares each with the PNG beside the cases. A pixel differs when any channel is more than 2 levels off (`CHANNEL_TOLERANCE`: room for another CPU's code generation rounding an anti-aliased edge; this machine is exact); a panel fails when more than 0.02% of its pixels differ (`MAX_DIFFERENT_SHARE`: 15 pixels of 320 × 240, less than one missing two-digit seat label). A failure prints the case, the panel, how many pixels and by how much, and what the case guards, and writes the drawn image and a difference image (differing pixels red) to `target/golden/actual/`. A missing baseline fails; `--update` writes baselines (the named cases, or all) and the Chromium version beside them (`chromium.txt`); a different version fails every case until they are redrawn and the image diff reviewed. `--runs n` draws everything in n separate browser starts and fails unless they match byte for byte. Cases, each a row of panels tiled into one image:

| Case | Panels | Guards |
|---|---|---|
| `levels` | `theatre` at overview (a sold-out section in grey with its second line, tints, the stage text), cross-fade, seats (strokes, hollow unavailable seats, row labels), detail (seat numbers from the baked atlas) | Each level of detail (04 §10) |
| `states` | One detail view in light, dark and high contrast: available, unavailable, selected and held, selected in flight, accessible, hover ring, focus ring | Each visual state (04 §9), each theme |
| `shapes` | Rounded and chair seats at detail with a cross and a tick; chairs at the seats level | Each seat shape (the circle is in every other case) |
| `scripts` | Section labels in Arabic, Hebrew and Greek with a text object mixing Arabic and Hebrew; Cyrillic row labels; lower-case seat labels | Text in Latin and other scripts, right to left, the dynamic atlas (04 §12) |
| `ratio` | A detail view at a pixel ratio of 2 | Device-pixel strokes, edges and distance-field labels (04 §21) |

The five PNGs total 317,841 B. Measured locally (AMD Ryzen 9 7900X, Chromium 155, SwiftShader software rendering): about 6 s for the five cases once built, 10 s with the incremental build; three browser starts drew identical bytes for every case. Not covered here: backend parity (no `wgpu` backend yet; a native renderer would read the same `cases.json`), the fallback ladder and context loss (13 §6.1 above, later slices), a hardware GPU, other browser engines, phones.

**Interaction traces** (`cargo test -p cavea-web traces`, about 1 s). Each `bench/traces/<name>.json` holds a chart, a viewport, a starting camera and state, and timestamped steps (pointer, wheel and key input, keyboard focus, hold answers); `crates/cavea-web/src/traces.rs` replays it through the engine on a virtual 60 Hz clock with a backend that draws nothing, and the events, encoded as the SDK receives them, must equal `<name>.events` line for line (`CAVEA_BLESS=1` writes them; a missing file fails). The traces: `tap-select-deselect`, `tap-unavailable` (`not_free`), `selection-limit` (`max`), `hover`, `drag-pan` (with inertia), `drag-not-tap` (6 px is a touch tap, 12 px a drag), `wheel-zoom-lod` (seats to detail and back across the hysteresis), `pinch-zoom-lod` (overview through the cross-fade to seats), `keyboard-focus` (D-52: arrows, Enter, Escape), `touch-zoom-to-select`, and `s2-gestures`, the benchmark's 30-second trace, checked as a summary (each event type's count and last event, camera changes per level). What the engine alone decides runs natively; the host's DOM wiring (`packages/renderer/src/host.ts`: pointer capture, wheel line deltas, `keydown`, `:focus-visible`) needs a browser and is not covered by these traces.

Both commands are local, never in CI: run them before filing a change to `cavea-render`, `cavea-web`, the shaders or the SDK's host and text code.

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

**Benchmark page** (`apps/demo/bench.html`, D-66). Runs S1 to S5 on a preset (default `stadium-100k`) through the public SDK, `runs` times each (default 10), and posts one JSON document (`apps/demo/src/bench-result.ts`: per-run figures, their median and 90th percentile, the browser, operating system, device model where the browser reports it, pixel ratio, viewport, battery, the commit) to a collector. S1 loads the map in a fresh frame (`bench-load.html`, the SDK alone) each run: the timing marks, chart-ready, and the files fetched with their transfer and encoded sizes; the frame gets a new WASM instance, but the browser's HTTP cache is warm after the first run and a page cannot clear it. S2 replays `bench/traces/s2-gestures.json` with real time, scaled to the map's size, as synthetic pointer events on the canvas; the frame time is the interval between animation frames as the page sees them (a frame over `budget`, default 20 ms, is over budget: a missed frame at 60 Hz), with the SDK's own work per frame from `onMetrics` beside it. S3 is S2 while a custom adapter applies 1,000 seat changes a second, timing each `applyDelta`. S4 taps 25 seats across the chart on and off, from the release to the next frame. S5, after each S2: the WASM memory, an estimate of GPU memory from the WebGL calls (buffers, textures, the drawing buffer; no driver overhead) and the JavaScript heap where reported. The chart is compiled in a worker with its own WASM instance, so the map's memory never held the compiler. Needs the `--demo` WASM, so the WASM S1 loads carries the demo's generator (about 680 KB against the shipped 302 KB raw): S1's payload is the page's, and the buyer's payload is what `cargo xtask build-web` measures (§6.3). Figures come from the production build (`bun run build` and `bun run preview --host` in `apps/demo`), not the dev server, whose modules are unbundled. On a phone: run the collector on a machine on the same network, `bun bench/collect.ts --listen <its address>:8787`, serve the built demo with `--host`, and open `http://<address>:<port>/bench.html?collector=http://<address>:8787&label=<device>` on the phone. The collector (`bench/collect.ts`) accepts only a POST of that exact shape (every key known, every value typed, strings and lists bounded) of at most 1 MiB, names the file from its clock and a counter, and writes it to `bench/local-results/` (gitignored); it binds to loopback unless told otherwise. Results from a development machine's software rendering are smoke runs, never reference figures (those go to `bench/results/`, TST-016).

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
| Golden images and interaction traces (§6.1) | Locally, before filing a change to the renderer, the shaders or the SDK's host | Any case differs |
| Device benchmarks S1–S5 | Every release | Any budget in `01-architecture.md` §5 is missed |

**Payload.** `cargo xtask build-web` measures the brotli size (quality 11) of what a buyer's browser downloads: the production bundle of `@cavea/renderer`'s public entry (`bun build --minify --target browser` with `process.env.NODE_ENV` defined as `"production"`, as a customer's bundler makes it: the SDK and the wasm-bindgen glue, tree-shaken), the WASM after `wasm-opt -Oz`, and the glyph atlas. It runs in CI's `rust` job, which installs bun for it (no `bun install`: the package has no dependencies). Measured 2026-11-11 with the SDK (WEB-001 to WEB-004 and part of WEB-005): bundle 36,361 B raw, 10,646 B brotli (of which the glue, bundled alone, is about 5.3 KB); WASM 83,949 B; atlas 9,462 B; total 104,057 B, against 98,593 B before, when the gate counted the raw glue (7,830 B brotli) instead of a bundle and the WASM was 81,306 B. With the rest of WEB-005 and `caveaServer` (WEB-020 to WEB-025): bundle 50,723 B raw, 15,453 B brotli (the methods 604 B, the adapter, its frame codec and the expiry callbacks 4,203 B); WASM 84,743 B (the methods' camera targets, 794 B); total 109,658 B. With dynamic text, section labels and text objects (REN-026): bundle 52,262 B raw, 15,911 B brotli (+458 B: the text rasteriser and its wiring); WASM 92,410 B (+7,667 B); total 117,783 B (+8,125 B). Of the WASM: the `text` program's shader text 0.8 KB; the label builder about 2.5 KB (section labels with the sold-out line, seat labels from the dynamic atlas, text objects, the build key and upload); the passes, `SetText`, `SetMessage` and the theme's font about 2.2 KB; the dynamic atlas and its packer about 1.0 KB; the bindings and the `textRequest` event's JSON about 0.6 KB (shares of the raw size by function, `twiggy diff`, applied to the brotli total). With row labels, collision and seat and decoration icons (REN-027, REN-028): WASM +3.2 KB, of which the icons' shader text about 1.0 KB (the stroke tables 0.4 KB) and the row labels, the collision pass, the icons' instances and the draw modes the rest. With context restoration and frame statistics (REN-040, REN-044): WASM +2.5 KB (resources made again from the engine's copies, the backend's `restore`, the `contextLost`, `contextRestored` and `frameStats` bindings), bundle +535 B (the context listeners, the 5 s report, the marks and `onMetrics`); total 123,835 B (+6,052 B). Drawing every row label from the dynamic atlas instead of baking the Latin ones (D-43) saved 1,104 B of that. With the keyboard and screen-reader baseline, the error model and preload (A11Y-001, A11Y-002, WEB-017, WEB-012): total 136,143 B (+5,317 B over 130,826 B): WASM +3,213 B (by `twiggy diff` shares of the raw growth: the focus search about 1.4 KB, `Key` with the camera's follow, zoom keys and the focus reports about 1.4 KB, the `key`, `setFocusShown` bindings and the row place in `object` about 0.4 KB); bundle +2,104 B (keyboard listeners, the proxy, names and live region about 1.3 KB; the error model, retries, `signal` and `preloadSeatMap` about 0.8 KB). The ring costs nothing: it is the existing overlay.

**Micro-benchmarks.** `cavea bench` (`cargo run --release -p cavea-cli -- bench [name…]`) measures on the `stadium-100k` preset: CCF decode; hit test (10,000 taps near seats at seat sizes of 18 to 64 px with the 12 px touch tolerance, time per test); the keyboard focus (`focus`: the search for 10,000 random seats and arrows, time per search, and one arrow key through the engine at overview and at detail, `11-accessibility.md` §3.2); snapshot apply (100,000 objects) and delta apply (10,000 changes), each from frame bytes through `Command::from_frame`, the engine and the next frame's buffer upload, on a backend that does nothing; seat labels of 4,096 seats (a full render chunk's worth) through `cavea_format::full_label::seat_label`; and compiling each preset. Each benchmark warms up for 0.2 s, then takes 5 to 15 samples of at least 10 ms each (short runs are batched) and reports the median and the minimum time per run, with the budget beside it where one has a hard number: decode 5 ms per 100,000 seats (`03-chart-format.md` §4.4), hit test 20 µs (`04-renderer.md` §15), focus search 20 µs and an arrow key 50 µs (`11-accessibility.md` §3), delta apply 2 ms (`01-architecture.md` §5), label build 1 ms (`04-renderer.md` §14, "well under a millisecond"). A missed budget exits 4. `--json` prints the same as JSON. It refuses to run in a debug build.

The gate compares a machine with itself: `--save <file>` writes the medians, and `--baseline <file>` compares a later run and exits 4 when any benchmark's median is more than 10% slower. A builder runs it with `--save` on `main` and `--baseline` on the branch before filing a change to the measured code; a flagged result is re-run before it is believed (the median of 5 to 15 samples absorbs most noise, but not another process taking the core). No baseline is committed and CI runs none: a number from one machine says nothing about another.

Measured locally, 2026-11-11, AMD Ryzen 9 7900X, Linux, release build, with no fuzzer running but other agents' work on the machine (load average about 5 on 24 threads); not reference-device results:

| Benchmark | Median | Minimum | Budget |
|---|---|---|---|
| decode (100,000 seats, 864,477 B) | 0.533 ms | 0.532 ms | 5 ms |
| hit test (per tap, seats at 18 to 64 px) | 0.296 µs | 0.293 µs | 20 µs |
| focus search (per arrow, from a random seat) | 1.23 µs | 1.22 µs | 20 µs |
| arrow key through the engine, at overview / at detail | 2.14 µs / 2.15 µs | 2.13 µs / 2.14 µs | 50 µs |
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

**Snapshot p99 on a fresh process, and a 60,000-seat chart** (measured locally on 2026-11-11, same machine and pinning as above, the shared PostgreSQL, load average 0.8 to 3.4; `--profile l2 --rate 500`, 20 to 30 s, whose 100 watchers fetch the snapshot within about 50 ms of each other at the start; each run a fresh server process and database). The p99 of those 100 snapshots missed the 3 ms budget: `stadium-60k` 8.96 and 12.29 ms, `arena` (20,000 seats) 9.98 ms; a second run on the same process 0.58 ms (`stadium-60k`) and 0.50 ms (`arena`). So the chart's size was not the cause: the snapshot is encoded once per publication and shared (08 §8.5), and the warm process served 60,000 seats as fast as 20,000. The server's own durations for the route (`cavea_http_duration_seconds`) put 9 to 12 of the first 102 requests between 2.5 and 10 ms: the pool had one connection, and every concurrent request of the first burst that needed one (the key lookup, the event lookup) opened it, a PostgreSQL backend and SCRAM authentication each; sqlx also closes connections idle for 10 minutes, so a quiet spell before an on-sale did the same. The pool now opens all `CAVEA_DB_POOL` connections at start and keeps them (08 §13). After, on fresh processes: `stadium-60k` p99 0.56 ms, `arena` 0.86 ms, 101 of 102 requests under 2.5 ms by the server's own count. The earlier report of 23 to 28 ms on `stadium-60k` was the same effect on a busier machine. 150 s of load in all.

**The snapshot pool fix, reviewed** (2026-11-11). Opening all connections at start stays; what changed is that only the first must open, so a database at its `max_connections` (for example while the process being replaced still holds its pool) no longer fails the start (08 §13), and a test covers a database that is down at start, one that admits only some connections, and recovery after an outage.

**Console files and holds** (measured locally on 2026-11-11, AMD Ryzen 9 7900X, 24 threads shared with two other builders, load average 1.3 to 3.9, release build, client bun's `fetch` over loopback, keep-alive, 8 at once): `index.html` p50 0.09 ms and p99 0.15 ms (88,000 a second); the 330 KB console script as brotli (86 KB) p50 0.16 ms, p99 0.47 ms; the designer's WebAssembly as brotli (567 KB) p50 0.97 ms, p99 3.8 ms, bound by the copy. Holds during console loads: `cavea-loadgen --rate 300 --duration-s 15 --subscribers 200` on `theatre`, alternately alone and beside 100 console loads a second (index, script, stylesheet, WebAssembly: 400 requests and 60 MB a second); hold p50 and p99 in four pairs, alone then with loads: 5.76/8.70 and 5.63/8.45 ms; 3.97/7.17 and 9.22/11.52 ms; 5.89/9.22 and 14.85/35.84 ms (loads first); 4.99/6.91 and 5.76/8.96 ms (loads first). Beside the loads holds were slower in three pairs (by 0.8, 5.3 and 9.0 ms at the median) and faster in one; runs alone varied by 1.9 ms at the median. The machine and its PostgreSQL, on which every hold waits for its commit, were shared with two other builders' work, so this check cannot tell the console's cost from theirs: an idle machine is needed to settle it. Serving a file touches no lock and no database and copies static bytes. A flood of 95,000 asset requests a second from six clients (8.1 GB a second) slowed holds plainly (p50 9.2 ms against 4.0 ms), as a flood of any route would on the same cores.

**The audit log's read route at 10 million rows** (measured locally on 2026-11-11, same machine, the shared PostgreSQL 18; 10,000,000 rows, 3.07 GB with indexes; 1,000 workspaces, one with half the rows, a tenth of the rows instance rows of 2,000 users; first page of 100, warm): the workspace with 5 million rows 0.3 to 0.4 ms (the first form of the query 0.3 to 0.5 ms); a workspace with 4,000 rows 0.4 to 0.7 ms (first form 21 to 29 ms); a workspace with one member and no rows 0.4 to 1.3 ms (first form 1.5 to 2.5 s, a backward scan of the whole table). The query now reads a page from each index, as 08 §5 says.

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
2. *Fixed since (below):* **a server whose log output fails stops serving.** When `tracing-subscriber` cannot write an event, it reports it with `eprintln!` (its `log_internal_errors`, on by default), and `eprintln!` panics when standard error cannot be written either. So with its log file's disk full, or a closed pipe, every task that logs panics: each request's connection is dropped without a reply when its record is written, the event's actor panics on its next log line, and the event is never reloaded (its load logs too). Found when the machine's `/tmp` filled during a PostgreSQL experiment: for 8 minutes the server answered almost nothing, and shutdown missed its deadline; nothing acknowledged was lost (reconciler clean, every acknowledgement in the log). Reproduced with the output sent to `/dev/full`: at the default level the server dies at its first log line; with `CAVEA_LOG=error` it serves. A fix is to turn `log_internal_errors` off and give the layer a writer that drops what it cannot write.

**Holds behind fan-out, found and fixed (D-45).** Measured 2026-11-11 on the same machine and core sets as above (server on CPUs 6, 7, 18 and 19, generator on 0 to 3 and 12 to 15, the shared PostgreSQL, default settings, `CAVEA_LOG=info` to a file), one experiment at a time, before on the code of #52 (`588820a`) and after on the server code of `103dcbc` (built before it was rebased onto `main`, which changed no server code; the L1 runs marked † on an earlier build that timed fewer commits for `cavea_commit_duration_seconds` and was otherwise the same). Load averages are of the whole machine at the start and end of each run.

*Where the time went.* `perf` was not available (`perf_event_paranoid` is 4 and there is no root), so the evidence is the server's own: a build with span-close events (an experiment, never committed) gave each stage's wall time. In the 25,000-subscriber stampede a public hold's p50 of 453 ms was spread over every await: authenticating the key 121 ms, finding the event 46 ms, the session binding 35 ms, the actor's reply 206 ms; each of those queries was 0.03 ms of work, and commits took 7 ms at the median. Hold latency rose to 800 ms with the server's cores 2.2 to 3.5 of 4 busy. Every await had waited behind a whole pass of 25,000 ready subscriber tasks in the one runtime's queue. Separating the streams onto a second runtime took that run's hold p99 from 799 ms to 112 ms; the actor then spent 13 ms per publication (p50; 33 ms at worst) waking 25,000 tasks on the other runtime, which a relay task there removed (22 ms at `CAVEA_LOG=warn`); nice 10 for the realtime threads gave 43 ms at the default log level, `SCHED_IDLE` 17 ms. Fanning out on loopback costs about 4 µs of user and 10 µs of system time per frame (per-thread figures from `/proc`), most of it the kernel sending and, on loopback, receiving each frame.

*L1* (55,000 connections):

| | Before | After | Budget or criterion |
|---|---|---|---|
| Hold p50 / p99 | 885 ms / 1,133 ms | 7.7 ms / 18.4 ms | p99 ≤ 15 ms: missed by 3.4 ms |
| Hold p99, other runs | 1,245 and 1,311 ms (above) | 17.9 ms; and 32.3, 34.8 and 106.5 ms† in runs where the machine was busier and the generator itself fell 5 to 10 ms behind its schedule | |
| Snapshot p99 | 2.69 ms | 3.39 ms (2.69 to 3.33 ms in the other runs) | ≤ 3 ms: at the line |
| Commit to delta delivered p50 / p95 | 225 ms / 401 ms | 737 ms / 1,114 ms | p95 ≤ 250 ms: missed, by design (below) |
| Frames written | 1,567,029 | 680,576 | |
| Server CPU in the buyers' seconds | 3.6 to 3.9 cores, all on one runtime | 3.3 to 3.7: requests 1.3 to 1.6, fanning out 1.8 to 2.7 | 4 |
| Server RSS, peak | 931 MB | 870 MB | 8 GB |
| Correctness | no violation, 55,000 views converged, no RESYNC, reconciler clean | the same in every run | |
| Load average | 3.97 to 5.42 | 1.82 to 2.24 | |

Hold latency no longer follows the number of subscribers. Holding the 5,000 buyers constant (each also subscribed) and adding watchers:

| Subscribers | Hold p99 before | Hold p99 after | Delta p95 before | Delta p95 after |
|---|---|---|---|---|
| 5,000 | 25.1 ms | 15.1 ms | 90 ms | 102 ms |
| 25,000 | 799 ms | 16.9 ms (35.8 ms in a run where the generator fell 30 ms behind) | 117 ms | 205 ms |
| 55,000 | 1,133 ms | 18.4 ms | 401 ms | 1,114 ms |

What gives instead is delta delivery: fanning out takes only the cores requests leave idle, and publications coalesce while the last is still on its way (08 §8.3). In L1, with 55,000 subscribers, a publication reached nine in ten of them in 395 ms on average (`cavea_rt_fanout_seconds`), and the generator saw publications about 500 ms apart. On loopback each frame's TCP receive also runs on the server's cores (§7.4, above), so a real network halves the cost per frame; whether 250 ms holds at 50,000 subscribers on 4 vCPU there needs that measurement. The remaining 3.4 ms over the hold budget is not fan-out: with only the buyers subscribed the same build measures 15.1 to 16.1 ms, and the actor's own figures in that L1 (from `/metrics`) were 5 to 10 ms at the median from a hold's submission to its reply, 99.4% within 15 ms; every one of 15,726 commands waited under 1 ms in the mailbox; 966 commits of 13 commands on average took 3.4 ms on average, 95% within 5 ms; the rest is three database lookups per request and commit latency on this desktop's storage (L2, above).

*L2 for 100 s* (2,000 arrivals a second, its profile otherwise; 100 s rather than 120 s because the host's connection-tracking table, shared with the other builders, fills at about 120 s, above): before, hold p50 7.04 ms, p95 11.3 ms, p99 42.0 ms (load average 2.18 rising to 6.25); after, 7.04, 9.5 and 15.1 ms (1.86 to 4.27). Delta delivery p95 96 ms both; 200,000 arrivals each, none skipped; no violation; reconciler clean; server RSS 47 to 166 MB before and 49 to 135 MB after, the live tokens of the first 100 s (60 s holds), as in the 600 s run above. The difference in p99 matches the difference in load; it is not evidence that this change moved L2's tail, which follows commit latency (above).

*The micro-measurements:* `measure_actor_throughput` (the actor against PostgreSQL, `tests/store.rs`) 45,669 commands a second before, 46,676 after; commit latency medians 1.64, 3.83 and 40.4 ms for 1, 100 and 2,000 commands before, 1.65, 3.73 and 42.2 ms after. `measure_realtime_publication` (the engine on the in-memory store): command to delta received p95 110 µs before, 93 to 102 µs after; per subscriber a 160-byte `Subscription` and an 88-byte pending `next()` both; commands a second with no subscriber 1.07 million before, 0.89 to 0.94 million after, the cost of the metrics where every command is its own batch on a store that commits at once (08 §15).

**The log output, fixed.** `tracing-subscriber` reported a failed write with `eprintln!` (`log_internal_errors`, on by default: confirmed in its source), which panics when standard error fails too. Records now go through a bounded queue (16,384) to a thread of its own that writes them in batches; one the queue cannot take, or the output cannot write, is dropped and counted (`cavea_log_records_dropped_total`), and nothing is reported on standard error. Standard input, output and error closed at start are held open on `/dev/null`, so that the first sockets do not take their descriptors. With standard output on `/dev/full`: before, the server exited with status 101 (a panic) at its first log line; after, it served a stampede of 2,000 watchers and 1,000 buyers (hold p99 11.8 ms, no violation, reconciler clean) and stopped cleanly; the same with standard error closed as well (hold p99 11.0 ms). The report's actor "that never reloaded" was the same defect: the registry catches an actor's panic and drops it so that the next request reloads the event (`a_panicking_actor_is_dropped_and_the_event_reloaded`), but its own `error!` after the catch panicked in the same way, leaving the event's entry behind; with logging unable to panic, that path has nothing that can. A request whose handler panics still gets no response (its connection is dropped) and is logged as nothing; no handler is known to panic.

**Console sign-in does not slow holds** (measured locally, AMD Ryzen 9 7900X, 24 threads, shared with two other builders; release build; loopback). One argon2id verification with the console's parameters (m = 19,456 KiB, t = 2, p = 1) takes 8.1 ms (median of 50; 12.8 ms the slowest) and grows the process's peak memory by 20 MiB (`console::password::tests::measure_password_verification`). Hold latency over HTTP (`cavea-loadgen`, arena preset of 20,000 seats, 300 arrivals a second for 20 s, 50 subscribers), against the same with sign-in attempts from a Python client on the same machine, two rounds interleaved after a warm-up:

| Concurrent load | Hold p50 / p99, ms |
|---|---|
| None | 5.5 / 8.2, 6.5 / 10.2, 6.3 / 10.8, 6.4 / 9.2 |
| 50 `GET /healthz` a second (the client's own cost) | 6.3 / 9.2, 6.1 / 9.5 |
| 50 sign-ins a second, refused by the back-off (`429`, nothing hashed) | 6.7 / 14.6, 6.3 / 10.0, 5.9 / 8.7 |
| 10 sign-ins a second, each verified (unknown emails, the dummy hash) | 5.6 / 16.9, 5.5 / 8.5, 5.9 / 10.2 |

Each kind's first round, run while the machine was busier, has the high p99; the repeats fall inside the baseline's spread. Every run held 6,000 times with no violation. Hashing runs on blocking threads, two at a time, so verifications never occupy the request runtime's workers.

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

**Bundlers, frameworks and page policies** (WEB-013 to WEB-016, WEB-018): `bun fixtures/run.ts [check…]`, from the repository root after `cargo xtask build-web`, on demand and never in CI. It builds `@cavea/renderer` and `@cavea/react` (`bun run build`), packs them (`bun pm pack`), and for each fixture in `fixtures/` (each outside the bun workspace, with its tools pinned in its own `package.json` and `bun.lock`) copies it to `target/tmp/compat/`, installs the lockfile with bun and unpacks the two tarballs into its `node_modules`, as a package manager does; builds run under Node (from PATH or `nixpkgs#nodejs`), as most customers run them. The page is the theatre preset (`cavea gen theatre`, compiled) with a custom adapter, served by a static server on a free port and loaded in headless Chromium (PATH or `nixpkgs#chromium`; WebGL2 on SwiftShader, software rendering) over the DevTools protocol, each visit in a fresh browser context. A check passes when the page's `onMetrics` reports the first frame with no error through `onError` and no policy violation; for the bundlers, also when the output holds the WASM as a file of its own and no script inlines it (no base64 `AGFzbQ`, no `data:application/wasm`). It records where the WASM and the atlas were fetched from. `csp-server` starts `cavea-server` (debug build) on a database of its own in the `deploy/compose.yaml` PostgreSQL, sets up a workspace, the chart, an event and a publishable key through the API, and drops the database at the end; the page is on `localhost`, the server on `127.0.0.1`, so they are different origins. The runner fails clearly when an install needs the network and has none, prints a table, and exits 1 if any row fails.

| Check | What it shows |
|---|---|
| `script-tag` | `cavea.min.js` by a script tag with `integrity`: the WASM and the atlas from beside the script (`/lib/v0/`), and from `assets.baseUrl` when the script is elsewhere |
| `vite`, `vite-dev`, `webpack`, `rollup`, `esbuild` | The shared page bundled with each, configured as 05 §13 says; the map draws from the files the bundler wrote |
| `next` | A Next.js App Router app: a server component page holding a client component with `<SeatMap>`, and a page whose client component imports `@cavea/renderer` with `import()`. `next build` prerenders both (the server render: an import that touched the DOM would fail it), the HTML holds the container, and the page hydrates and draws under `next start` |
| `react18` | `@cavea/react`'s own tests (`packages/react/src/index.test.ts`) under React 18 instead of the workspace's 19 |
| `csp-custom`, `csp-server`, `csp-cdn` | The policies of 05 §10 with `require-trusted-types-for 'script'`: the map draws with no violation; then each source is taken away in turn and the map must fail the way 05 §10 says. `csp-cdn` also serves a WASM with one byte changed from the CDN, which the script's pinned hash must refuse |
| `server-sdk` | `@cavea/server-sdk` (05 §15) packed and installed into a Node project (`fixtures/server-sdk`, TypeScript 5.9 and `@types/node` pinned): `tsc` type-checks it strict, without the DOM library and with `skipLibCheck` off, then the compiled program runs under Node 22 and 24 from nixpkgs against `cavea-server` (the `csp-server` setup): a workspace by the admin key, the theatre chart published, every free object walked, hold, book, an idempotent replay, a conflict caught by its code, a session token signed. Needs no WASM build when it runs alone |

**Measured locally** (2026-11-11; Linux; Chromium 157.0.8103.38 headless, SwiftShader; Node 24.21.0; bun 1.4.2). Every row passed:

| Check | Version | Result |
|---|---|---|
| script-tag | `cavea.min.js` | First frame; `/lib/v0/cavea_web_bg.wasm`, `/lib/v0/glyphs.png` |
| script-tag with `assets.baseUrl` | `cavea.min.js` | First frame; `/static/cavea/cavea_web_bg.wasm`, `/static/cavea/glyphs.png` |
| vite (build) | Vite 8.3.8 | First frame; `/assets/cavea_web_bg-<hash>.wasm`, `/assets/glyphs-<hash>.png` |
| vite-dev | Vite 8.3.8 | First frame; `/node_modules/@cavea/renderer/cavea_web_bg.wasm` |
| webpack | webpack 5.112.2, webpack-cli 7.3.1 | First frame; `/<hash>.wasm`, `/<hash>.png` |
| rollup | Rollup 4.68.2, node-resolve 16.0.3 | First frame; `/cavea_web_bg.wasm`, `/glyphs.png` (copied by the fixture's plugin) |
| esbuild | esbuild 0.28.2 | First frame; `/cavea_web_bg.wasm`, `/glyphs.png` (copied) |
| next `/` and `/direct` | Next.js 16.4.6, React 19.3.1 | Built, server-rendered with the container, hydrated, first frame; `/_next/static/media/cavea_web_bg.<hash>.wasm` |
| react18 | React 18.3.1 | 6 of 6 tests |
| csp-custom | — | Draws; without `'wasm-unsafe-eval'` `wasm_unavailable` at once; without `connect-src 'self'` `wasm_unavailable` after the retries |
| csp-server, transport `auto` and `sse` | `cavea-server` | Draws, realtime frames arrive (WebSocket; EventSource), and the chart's background image is fetched from `/v1/public/assets/{hash}` under the same `connect-src` with no `img-src` (REN-020); without the server's origin `chart_fetch_failed` or `state_connect_failed`; without its `ws:` origin, EventSource after two refused sockets |
| csp-cdn | — | Draws from the CDN's files; without `'wasm-unsafe-eval'` or the CDN in `connect-src`, `wasm_unavailable`; a changed WASM on the CDN, `wasm_unavailable` |
| Trusted Types | — | No violation under `require-trusted-types-for 'script'` in any CSP row |
| server-sdk | Node 24.22.2 and 22.23.4, TypeScript 5.9.3 | Types resolve; 2,000 free objects walked, 2 booked, the replay the same `seq`, `object_not_free` caught, a three-part session token. Node 20 is not proven: it left nixpkgs at its end of life (2026-04-30) |

The `csp-server` check found that Chromium fires only `error`, never `close`, on a WebSocket a policy refuses, which left `caveaServer` waiting 60 s for silence before trying again; it now counts the `error` (05 §4.1). These are Chromium results in software rendering on one machine, not other browsers or GPUs.

## 9. Designer

- **Command inverses:** for random command sequences, applying then undoing every command returns the original document, byte for byte.
- **Tools:** scripted pointer traces produce expected documents.
- **End-to-end:** draw a block, curve it, number it, assign a category, publish, create an event, hold a seat, and see it in the event manager: the end-to-end check below (§9.1), which covers the product's whole first path.
- **Performance:** scripted drag of 5,000 seats on the 100,000-seat chart, measured as in §6.2.

### 9.1 The end-to-end check

`cargo xtask e2e [--keep] [--headed] [--to n]` (TST-031, D-76) drives the product's first path in one run, as people use it, against what a release ships, and asserts on what it finds. Run it before filing a change to the console, the designer, the server's routes or the SDK; CI does not run it (§12).

**What it runs on.** It always builds first, so a stale binary is never tested: `cargo xtask build-server` (the designer's and the buyer's WASM, the second through the payload gate, the console, and `cavea-server` in release with the console compiled in, under its own Content Security Policy) and `@cavea/renderer` as published (`bun run build` in `packages/renderer`). With nothing changed that costs about 70 s on the machine below (the designer's `wasm-opt` and brotli about 27 s; the console's files compressed at brotli 11 and the server linked again with them most of the rest), then `bun tools/e2e/run.ts --server <binary>`. The runner copies the binary into `target/e2e/<run>/` (a build during the run cannot change what the restart starts), creates a database of its own (`cavea_e2e_<pid>_<random>`) on the `deploy/compose.yaml` PostgreSQL, starts the server on a free port with a random admin key, serves the buyer's page (`tools/e2e/buyer/`) on another free port, and starts the flake's Chromium (`nix build .#chromium .#golden-fonts`, the golden images' browser: headless, WebGL2 on SwiftShader) with a profile of its own under `target/e2e/<run>/`. The console is `http://localhost:<port>`; the buyer's page is `http://127.0.0.1:<port>`, another origin, under the policy 05 §10 gives a page using `caveaServer` (`default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'` and the server's `http:` and `ws:` origins; `require-trusted-types-for 'script'`). Every person is a browser context of their own (cookies and storage). On every way out, Ctrl+C included, it stops the server and the browser by process id and drops the database; two runs at once do not meet. The browser driver (`tools/chromium.ts`) is shared with the golden images and the compatibility checks.

**How it acts and looks.** Elements are found by role and accessible name in the browser's accessibility tree (`Accessibility.queryAXTree`), with the words from the console's own catalogue (`apps/console/src/i18n/messages.ts`), so a changed string moves the check with it. Input is real mouse and key events through the DevTools protocol, at coordinates read from the page: an element's box, the canvas's box, the map's own report of the seat under the pointer. Every wait is a condition with a timeout that names what it waits for; the one timed wait is the map's double-tap window (`DOUBLE_TAP_MS` in `crates/cavea-render/src/tuning.rs`, read from the file) between two clicks on the same seat, which a person's pace always leaves. WebGL contents are not compared: the canvases are checked through what the page says about them (the designer's status bar, inspector, panels and save state; the map's hover and focus reports and its screen-reader proxy) and through the server's own answers.

| Step | What it does | What it checks |
|---|---|---|
| 1. First run | Reads the setup link from the server's log, opens it, creates the owner | The link is on the console's origin; the page's heading; the signed-in frame (the account menu with the email) on the charts list; the token gone from the address bar |
| 2. Draw | Creates a chart from the charts list, opens it in the designer from the card's menu, skips the tour, chooses the row block tool and drags on the canvas; types `4x10` and Enter during the drag (07 §4, numeric entry, so grid snapping cannot change the counts); curves it (the inspector's curve: arc, radius 1500); numbers the seats from 101 (numbering panel, Apply); adds the category Stalls and assigns it | The live readout while dragging; 40 seats in the status bar; the curve field reads Arc; the numbering preview and then the current labels read `A 101 102 … 110`; the category counts 40 seats; once validation has finished (the panel no longer checking), no error (two warnings: no focal point, no accessible seat); the top bar says Saved; the draft on the server is one block of 4 rows of 10, an arc of radius 1500, seats from 101, category `stalls` |
| 3. Publish | Publishes from the designer's top bar, back to the console by the designer's menu, opens the chart's page | The dialog says version 1, 40 seats; the versions table has version 1, current, 40 seats |
| 4. Event | Creates the event from that version's row on the chart's page | The key made from the name; version 1 chosen; the form closes with the notice |
| 5. Keys | Creates a secret key and a publishable key (allowed origin: the buyer's page) in the API keys screen | Each key read from the dialog that shows it once (`sk_…`, `pk_…`); with the secret key, the event is on version 1 with 40 free seats |
| 6. Buy | Opens the buyer's page twice, in two contexts; buyer 1 clicks seat `-B-105` on the map (centred with `zoomTo`, found under the pointer by the map's hover report), ten times holding and nine times releasing it; then the customer's backend books it with the secret key (`fetch`; the Node server SDK is not on `main`) | Both maps drawn, each realtime over the WebSocket; after each click, buyer 2's map shows the seat unavailable (or available again) without a reload, buyer 1's selection holds it with a hold token, and the server says `held` (or `free`); after booking, `booked` on the server and unavailable on buyer 2's map. The latency of each (below) |
| 7. Operate | Opens the event's manager; reaches seats with the arrow keys from the map's keyboard focus, reading where it is from the map's screen-reader proxy (D-52); Enter selects | The details say row B, seat 105, booked; two free seats behind it selected and blocked: the notice, `blocked` on the server, unavailable on buyer 2's map; the booked seat released after the confirmation: the notice, `free` on the server, available on buyer 2's map |
| 8. People | The owner invites a viewer (Settings, Members); the link is opened in a new context, a password set | The invitation link on the console's origin; the invitation page's heading; the viewer signed in; the event's map shown, with the viewer's note and no Block, Unblock, Book or Release |
| 9. Restart | Closes the pages holding live connections, stops the server with SIGTERM, starts it on the same database | Exit 0 within 30 s, "stopped" logged; the event's counts as before (2 blocked); the chart published at version 1 with 40 seats; the owner's session still valid and the members table listing the owner and the viewer |
| 10. Throughout | Reads the server's whole log | None of the run's secrets in it: the admin key, both passwords and emails, both session cookies and CSRF tokens, both API keys, the hold token, the invitation's token (the setup link is logged by design, 09 §2) |

**Throughout**, after every step, each open page must have reported nothing the step did not expect: no uncaught error or unhandled rejection, no `console.error` or browser error log, no Content Security Policy violation (a listener installed before the page's scripts, and the browser's own report), no error the map reported through `onError` (on the buyer's page), and no failed request. The failed requests allowed, each by name: `GET /console/api/me` answering `401` once on a page that is not signed in yet (the setup page, the invitation page); and requests the page cancelled itself (an `AbortController`, a page left). The buyer's page server answers `/favicon.ico` with `204`.

**Reading a failure.** The run stops at the first failing step and prints its number and name, what it expected and what it found (a timeout names the condition and the last text it read from the page), then everything the pages reported meanwhile. Under `target/e2e/<run>/` it leaves, for each open page, `step<n>-<page>.png` (a screenshot), `.tree.txt` (the accessibility tree as roles and names: what a step can find), `.console.txt` and `.network.txt`, and the server's whole `server.log`. Everything printed or written, but the server's log and the screenshots, has the run's secrets replaced by their names (`‹secret key›`); a screenshot shows what the page showed. `--to n` stops after step `n`; `--keep` leaves the server up after the steps, with the sign-in details in `target/e2e/<run>/keep.txt` (mode 600), until Ctrl+C; `--headed` shows the browser (needs a display). A step that fails because of a product bug that is written up but not fixed can be marked `known` in `tools/e2e/steps.ts` with where the write-up is: it is then reported as a known failure and the run goes on to the next step (exit 4 when that is all); once it passes, the run fails until the mark is removed. Exit codes: 0 passed, 1 a step failed, 2 usage, 3 the check could not run (no PostgreSQL, no browser, the server did not start), 4 known failures only.

**Measured locally** (2026-11-12; AMD Ryzen 9 7900X; Chromium 155.0.8059.84 headless, SwiftShader software rendering; loopback). Ten consecutive runs of `cargo xtask e2e` on the final head: 10 passed, each 83 to 89 s in all, of which the steps took 15.5 to 16.3 s. Time by step, in seconds, the same in every run within 0.2: 1. first run 0.6 to 0.8; 2. draw 3.7 to 3.9; 3. publish 0.4; 4. event 0.2; 5. keys 0.7; 6. buy 6.9 to 7.0 (19 clicks a person's pace apart, by the double-tap window); 7. operate 1.1 to 1.2; 8. people 1.0; 9. restart 0.3 to 0.5 (the server stopped in 0.01 s); 10. the log under 0.1; the rest is starting the browser and the server. Three runs started two seconds apart all passed, and two on the final head. Before the check waited for validation to finish, a run started beside another failed at step 2 with "1 errors" from the last full check before the category was assigned: the check's race, not the product's (the panel says it is still checking).

**Two buyers, one seat** (M2's exit criterion: two browsers see each other's selections become unavailable within 250 ms). The time from buyer 1's press on the seat (the pointer event's time) to the first frame of buyer 2's map with the seat in its new state (a `requestAnimationFrame` loop reading `findObject`, so to a frame, 16.7 ms), both as Unix times on this machine, realtime over the WebSocket, the default server settings. Over the ten runs: every hold but each run's first seen by the other buyer in 17 to 67 ms (90 holds: median 25, 90th percentile 33 ms), the first hold of each run in 124 to 183 ms (median 133), a release in 17 to 54 ms (90: median 26, 90th percentile 38). None of the 190 took longer than 250 ms. The first hold of each run is the slowest: it also creates the buyer's hold token (a preflight and a `POST` before the hold). Measured locally, AMD Ryzen 9 7900X, Chromium 155, SwiftShader software rendering, loopback: one machine, no network between the browsers and the server, two buyers.

**It fails when the product is wrong.** Each of these was planted, run with `cargo xtask e2e`, and reverted:

| Planted | Layer | The run |
|---|---|---|
| The manager's actions send each seat's neighbour (`live.ts`: label number + 1) | Console | Step 7: "expected -C-105 blocked on the server; found free" |
| `POST /v1/events/{event}/book` answers `409` after booking (`http/seats.rs`) | Server | Step 6: "expected POST /v1/events/opening-night/book to succeed; found 409 {…}" |
| Applied seat numbering steps by one more than asked (`cavea-edit` `numbering.rs`) | Designer (Rust, WASM) | Step 2: "timed out after 15 s waiting for the preview to read A 101 … 110" |
| The book handler logs the hold token | Server | Step 10: "expected none of … in the log; found hold token" |
| The buyer's page policy without the server's `ws:` origin | Page | Step 6: no WebSocket, and the pages also reported the `connect-src` violations (`csp connect-src ws://localhost:…`) |

**What the run found.** The console named no icon, so every console page load asked for `/favicon.ico` and got `404`, a failed request in every browser's log; it now names its own (`/assets/favicon-….svg`) and the allowance is gone. The setup, sign-in and invitation pages showed their title without a heading role; the title is now a heading (`ui/form.tsx`, `Card`).

**What it does not cover.** It is one browser engine (Chromium) with software rendering on one machine. It does not check what the canvases look like (the golden images, §6.1, do), a hardware GPU, other browsers, phones or touch, load (§7), the stream's fallbacks to EventSource and polling (the compatibility checks' `csp-server`, §8, does EventSource), more than one workspace, or what a person finds hard; it does not replace the unit tests or a person using the product.

## 10. Security tests

- **Authorisation matrix:** a generated test calls every endpoint with every credential type and with another workspace's identifiers, and compares the status with a declared table. **Implemented** (TST-026) as `authorisation_matrix` in `crates/cavea-server/src/http/tests/api.rs`, against PostgreSQL. Its rows are `ROUTES`, one per operation of the route table that builds the router and the OpenAPI document (08 §6.1); `every_route_has_a_row` (no database) fails while a registered route has no row or a row no route. Each row's credential comes from the route table. Its columns: none; three malformed forms (`Basic …`, `Bearer` alone, `Bearer a b`; in a URL, a percent-encoded, empty or repeated `key`); three invalid keys (never issued, too short to be one, revoked); the admin key; the workspace's secret and publishable keys; another workspace's secret and publishable keys; and console sessions (08 §11): one per role of 09 §2 (owner, admin, editor, operator, viewer) naming the workspace, an owner of another workspace naming theirs, and another owner of that workspace naming the tenant's, which they are not in. The declared table (`expect`) gives each cell's status and `reason`: `401` `missing`, `malformed` or `invalid`, and `console_session` on the console routes without a session; `403` `key_kind` for the wrong kind (admin key on tenant routes, tenant keys on admin routes, secret where publishable is needed and the reverse); for a member, success exactly where 09 §2's role table grants the route's declared permission (`x-cavea-permission`, from the route table), else `403` `role`; on the members routes, which take a console session only, a key or no session is `401` `console_session`; `403` `workspace` for the member naming a workspace they are not in; a cookie is no credential on admin or public routes (`401` `missing`); and success, or `404` for another workspace's key or member of the right kind when the route names an identifier of the tenant's, whose answer must reveal none of them. On every non-safe route a console session takes, the owner's session without its CSRF token (`403` `csrf`), from another site (`Sec-Fetch-Site: cross-site`, `403` `origin`) and, on `/v1`, without `Cavea-Workspace` (`400` `workspace`). Sign-in, sign-out and a password change end the sessions they are sent with, so the matrix starts new ones after those rows. Routes that take no credential answer every column alike. Then, on the three routes that take `Cavea-Session`, an expired token and one another workspace signed (`401`, `session_token`, `expired` and `invalid`); and where a body names a hold token for the event in the path, another event's token (`404`, `missing: "hold_token"`). Over 1,000 cells; every response is also validated against the OpenAPI document, and the log of the whole run must hold no key, token, password, email, console session identifier or CSRF token. Origin allow-lists and an unset admin key are in `origins_and_unset_admin_key`. Two tests without a database hold the declarations to 09 §2 by hand: `every_route_declares_its_permission` lists every route's permission and fails when a route is added, removed or remapped without the list; `the_role_table_is_09s` writes the role table out bit by bit. The removal row's member is put back before each cell, so every allowed cell removes one.
- **Members** (`http/tests/api/members.rs`): a lowered role and a removal apply to the member's next request with the same session, which stays valid (`me` lists no workspace; signing in still works); 160 random role changes, removals and returns by random members, each answer compared with a model of 09 §2's rules (`settings`, `ownership` to make or unmake an owner, `last_owner`, `404` for a non-member), the database compared with the model after each, and an owner left after every step.
- **Tenant isolation:** two workspaces with identical keys for charts and events; no call may cross. The matrix's other-workspace columns use distinct keys; `identical_keys_never_cross` gives both workspaces the same chart and event keys and different charts: every read naming only those keys answers each workspace with its own and none of the other's identifiers, a hold in one is not in the other's counts or objects, and archiving one's chart leaves the other's.
- **Console sign-in** (`http/tests/api/console.rs`, 12 §3 T9): setup creates one owner under a race, burns the token, logs the link once and nothing once an owner exists; an unknown email, a wrong password, a malformed email and an over-long password answer identically; the cookie's attributes; rotation at sign-in, revocation at sign-out and on a password change; sliding renewal, the five-minute write throttle, the idle and absolute ends; the back-off doubling per (account, address), the same for an email nobody has, never against the member's own address, the per-address budget; a console user as the event log's actor and as the author of drafts and versions. The image walkers take 20,000 seeded mutations of valid files without a panic (`assets.rs`).
- **Invitations and the audit log** (`http/tests/api/members.rs`): a link works once, before it expires, for its own email (a new user sets a password and is signed in; an existing one must be signed in with the CSRF token; another account's session is refused and leaves it); revoked links stop; an admin cannot invite an owner and the admin key can; two acceptances of one link at once give one member, one user and one row; a member's email cannot be invited, and a link opened by someone who became a member meanwhile keeps their role; no token, link or email in the log. One audit row per committed action, in order, with identifiers only; none for an idempotent repeat or a seat command; a failing audit write undoes its action; a failed sign-in writes a row only for an account; the workspace's creation by the admin key has its row; the read route's pages. The routes' credentials and roles are the matrix's.
- **The console's files** (`http/tests/web.rs`, a fixture table, no built console): cache headers per kind, brotli by `Accept-Encoding` with `Vary` and its own `ETag`, `application/wasm`, `304` for `If-None-Match` (weak and `*` too), `HEAD`, `index.html` for navigations only, a missing asset and traversal attempts `404`, API paths keeping problem+json, other methods `404`, the same answer whatever credential is sent and no `no-store` for the cookie; without a built console, its paths say so.
- **The pool** (`tests/store.rs`): opens every connection at start; a database down at start stops it; one admitting fewer connections than the pool's size does not; after an outage, queries work again.
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
- [ ] The image builds for x86-64 and arm64 (`docker buildx build -f deploy/Dockerfile --platform …`), and `deploy/smoke.ts` passes against it: setup link, owner, chart, event, hold and book, the console's files and headers, the reconciler, a clean stop within the grace period, the data after a restart, no secret in the log.
- [ ] Images and packages signed; SBOM attached.
