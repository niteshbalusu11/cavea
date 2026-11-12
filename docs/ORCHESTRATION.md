# Orchestration playbook

**Status:** v1 · 2026-11-10

How this repository gets built: one orchestrator thread hands slices of `docs/TODO.md` to build agents, each of which ships its slice to `main` through a reviewed pull request. This file is the orchestrator's standing instructions. If your context is ever summarised, re-read this file, `AGENTS.md` and `docs/TODO.md`, then run `gh pr list` to rebuild your picture of what is in flight.

## 1. Roles

| Role | Model | Does | Does not |
|---|---|---|---|
| **Orchestrator** | Claude Fable 5.2, high effort | Chooses slices, writes briefs, starts builders, runs the merge gate and merges, keeps `docs/TODO.md` honest, reports at milestone boundaries | Write product code, review code, merge anything that has not passed the gate |
| **Builder** | Claude Opus 5.5, high effort, "Claude Work" provider | Implements one slice in its own worktree and takes it to a reviewed, proven, open pull request | Start other builders; approve its own work; wait on CI or merge |
| **Reviewer** | GPT-6.1 Sol, high reasoning effort | Reviews a builder's commits through the `sol-loop` skill, started by the builder | Edit code |

The orchestrator only orchestrates. If something needs a code change, however small, it goes to a builder.

**Why the orchestrator merges.** A delegated builder is not allowed to watch or poll a pull request, so it cannot wait for CI. The scaffold builder proved this: it stopped with its pull request filed, approved by Sol and unmerged. Builders therefore finish at "pull request open, proof gathered, Sol approved", and the orchestrator, which can watch pull requests, runs the merge gate (§7). The orchestrator may commit documentation-only changes (ticking the task list, correcting a spec, this file) through a pull request of its own.

## 2. Goal and stopping point

- Work through `docs/TODO.md` in the order of `docs/ROADMAP.md`, milestones **M1 to M4**: format and compiler, web renderer and SDK, server and realtime, designer, console, licensing, documentation.
- The aim is a working product: draw a chart in the designer, publish it, create an event, hold and book seats from a browser, operate it from the console.
- **Stop after M4.** Do not start M5 or later without the owner saying so.
- Report to the owner at each milestone boundary (§8).

## 3. What to skip

- Every `VAL` task, and anything that needs a person: outreach, interviews, legal review, buying devices, usability sessions, screen-reader sessions with real users.
- M0's business tasks. Its rendering-spike tasks (`REN-001` to `REN-006`, `FMT-001`) are absorbed by the M1 tasks that supersede them; tick them when the M1 equivalent merges. Treat `REN-002` (build-time shader translation) and `REN-003` (measured payload size) as real, early tasks, because two design decisions depend on them (`docs/DECISIONS.md` D-03, D-05).
- Measurements on reference phones. Agents measure what this machine can: payload size, micro-benchmarks, headless frame times, load tests against local PostgreSQL. Record such results as "measured locally" and never present them as device results.

## 4. Starting a builder

Call `orchestrator_capabilities` once, then for every builder use `delegate_task` with:

| Setting | Value |
|---|---|
| `target.providerInstanceId` | `claudeAgent_claude_work` |
| `target.model` | `claude-opus-5-5` |
| `target.options` | `{ "effort": "high" }` |
| `isolation` | `worktree` |
| `role` | `implementation` |
| `mode` | `async` |
| `title` | Short name plus the task ids, for example `CCF writer and reader (FMT-020..022)` |

Follow each task's returned `next`. Do not poll running builders; their completion wakes you.

A builder knows nothing except its brief and the repository. Every brief must contain:

1. **Where and what:** the repository, that it works in its own worktree on a branch, and the exact task ids.
2. **Authority:** "Full stop" — it must load and follow the `full-stop` skill (the `full-send` workflow without the merge): build, prove locally, review with `sol-loop`, rebase, file the pull request. It must not wait for CI and must not merge. It is allowed to start `sol-loop` review delegates using GPT-6.1 Sol (`gpt-6.1-sol`, codex provider) at high reasoning effort, and no other subagents.
3. **What to read first:** `AGENTS.md`, then the specific spec sections for its tasks.
4. **Scope and boundaries:** which crates and directories it owns for this slice, which it must not touch, and any interface another in-flight builder depends on.
5. **Definition of done:** the behaviour that must work, the proof to gather, and that it ticks its task ids in `docs/TODO.md` in the same pull request.
6. **Constraints:** smallest change that completes the slice; tests only where they have real value; `CARGO_BUILD_JOBS=8`; bun only; shared PostgreSQL with uniquely named databases; stop anything it started.
7. **Attribution lines** for commits and the pull request, as `AGENTS.md` and the harness specify.
8. **Final report format:** what was built, decisions later builders must know, anything left undone, and a last line `PR <full URL>, head <SHA>, checks and reviews <state>`.

## 5. Choosing slices

- A slice is 3 to 8 related task ids that make one reviewable pull request. Split anything that would exceed roughly 1,500 changed lines.
- **Builders running at the same time must not edit the same crate or package.** Shared files (`Cargo.toml` workspace dependencies, `Cargo.lock`, `bun.lock`, `docs/TODO.md`) will conflict occasionally; the later pull request rebases.
- When two slices need a shared type or interface, land the interface first in a small slice, then fan out.
- Respect dependencies inside a milestone: types before algorithms, format before renderer loading, engine before server endpoints, edit core before tools.
- Milestones are ordered, but work on disjoint crates may be pulled forward to keep builders busy. The seat engine and server (M2) do not depend on the renderer (M1), so they can run alongside it. The designer (M3) needs the compiler and the renderer.
- Do not hand out a task whose spec is unclear. Fix the spec first, in a docs pull request.

A sensible first wave once the scaffold is on `main`:

| Builder | Slice | Owns |
|---|---|---|
| A | Chart document types, label schemes, row-block expansion: `FMT-002` to `FMT-009` | `cavea-compiler`, document types in `cavea-core` |
| B | Engine protocol, backend trait, WebGL2 backend, shader build pipeline, seat program on synthetic data: `REN-002`, `REN-003`, `REN-010`, `REN-013` to `REN-017`, with `FMT-001` in the testkit | `cavea-render`, `cavea-web`, `cavea-testkit` |
| C | Seat engine domain types, store trait, in-memory store, actor loop: `ENG-001` to `ENG-005` | `cavea-engine` |
| D (small, start first) | Nix development environment: `FND-010` | `flake.nix`, `flake.lock`, `.envrc`, the tooling notes in `AGENTS.md` and `README.md` |

Slice D in detail: a `flake.nix` dev shell that provides every tool the repository needs beyond a bare OS (the Rust toolchain exactly as pinned in `rust-toolchain.toml` including the `wasm32-unknown-unknown` target, no separate `wasm-bindgen-cli` since `xtask` uses the pinned `wasm-bindgen-cli-support` crate, `binaryen` for `wasm-opt`, `brotli`, `bun`, a PostgreSQL client, and anything else the scaffold installed by hand), plus an `.envrc` containing `use flake`, so that `cd` into the repository followed by `direnv allow` loads everything. Commit `flake.lock`; ignore `.direnv/`. Proof: in a fresh shell, `nix develop -c` runs every command in the `AGENTS.md` "Commands" section successfully. CI does not use Nix; it keeps its plain cached toolchain setup, and the flake must stay in agreement with it.

### What the scaffold established

Carry these into briefs where relevant:

- `wasm-bindgen` and the `wasm-bindgen-cli-support` crate used by `xtask` are pinned to the same version. Bump both together.
- Run `cargo xtask build-web` before `bun run typecheck`, `build` or `test`; the WASM output is gitignored.
- Vite runs under the bun runtime; Node is not needed. `@cavea/renderer` ships TypeScript source directly.
- CI is one workflow: a `rust` job (format, clippy, build, `check-deps`, `build-web`) that uploads the WASM as an artefact, then a `web` job. The Rust cache is saved only from `main`.
- The machine's Nix-provided Rust has no `rust-lld`, and binaryen is not installed. The flake (`FND-010`) provides both; builders run everything as `nix develop -c <command>`.
- CI duration on `main`, measured 2026-11-11 on the scaffold commit: first run 1m16s (rust 57s, web 14s); cached rerun 35s (rust 20s, web 13s).

### What the first waves established

Carry these into every brief:

- **Run the bun checks on Rust-only slices too.** `bun run format:check` (oxfmt) also checks TOML, so a hand-edited `Cargo.toml` can fail CI's `web` job. Every brief lists `bun install --frozen-lockfile`, `bun run lint` and `bun run format:check` next to the cargo checks.
- **Stop processes by process id only.** A builder once ran `pkill` with a pattern that matched every Vite dev server on the machine. Briefs forbid `pkill` and `killall` with patterns.
- **Slices run large.** Builders report 2,000 to 2,600 changed lines for slices briefed at 1,500, of which a third or more is tests. Name the natural cut in each brief; a builder that splits must say where the unreviewed remainder is, and a follow-up builder takes it through review.
- **A builder's brief lists what exists.** Each brief names the modules, types and decisions already on `main` that the slice builds on, taken from the previous builders' final reports. Builders start from nothing else.
- **Rebasing a waiting pull request** onto a new `main` is done with `gh pr update-branch <n> --rebase` when there is no conflict; check with `git range-diff` that the commits are unchanged, so the reviewer's approval still covers them, then wait for CI on the new head. A conflict goes to a builder.
- **Browser proof** works with Chromium from Nix (`nix shell nixpkgs#chromium`), headless, WebGL2 on SwiftShader, driven over the DevTools protocol. It is software rendering: say so. Firefox's headless screenshot fires before the WASM starts and proves nothing.
- **Determinism of compiled charts.** No platform maths library call anywhere on the compile path; `cavea-compiler::detmath` supplies sine and cosine. A pinned-hash test runs natively and for wasm32 under bun (command in the test's doc comment). Not automated; `TST-014` is open.
- **The shared PostgreSQL container** (`cavea-postgres-1`, compose project `cavea`, port 5432) is started by the orchestrator and left running while server work is in flight. Builders never run `down` on it; database tests run only when a test database URL variable is set, and are reported as skipped otherwise.
- **A builder can vanish without filing.** Two ways it has happened (2026-11-11): a builder's final report was never delivered as a wake although its pull request was open; and a builder ended its turn to wait for a background test run, was never woken, and its worktree was removed with eight unpushed, reviewed commits (the branch survived only as a local ref in the shared repository). So: briefs tell builders to push early and often and never to end a turn with a command running; when a pull request appears without a report, or a builder has been silent for an hour, read the task's status and, if it is finished, its thread's last messages (`t3_thread_read`); look for its branch among the local refs (`git branch --list`, a `[gone]` upstream) before assuming the work is lost, push it under an `-unfiled` name, and start a finishing builder that rebases, gets the rebased slice reviewed, proves it and files.
- **`/tmp` fills up.** It is a 15 GB memory-backed filesystem; builders' scratch directories and reviewers' checkouts filled it once (2026-11-11). Briefs tell builders to set `TMPDIR` inside their worktree and to clean up; between waves the orchestrator removes the scratch directories of finished builders whose worktree is gone (`/tmp/claude-1000/*worktrees*<project>*` and `/tmp/cavea-*`, untouched for two hours or more), and nothing that belongs to another project.
- **Crate ownership so far:** document types and the compiler pipeline are in `cavea-compiler`; the label engine and limits are in `cavea-core`; the compiled `Chart`, `encode` and `decode` are in `cavea-format`, which computes no hash (the compiler does); camera maths sits in `cavea-render` for now.

## 6. Concurrency and the machine

- **At most three builders at once.** The machine has 24 cores and about 18 GB of free memory, and each builder caps cargo at 8 jobs.
- Drop to two if builds start failing for lack of memory or the machine thrashes.
- GitHub Actions is not the constraint (public repository); local compute is.
- Between waves, check for leftovers: stray containers (`docker ps`), dev servers, and disk used by old worktrees' build directories.

## 7. When a builder finishes

1. Read its final report. Link the pull request to your thread and watch it with the harness's pull-request tools; do not poll or sleep.
2. **Run the merge gate.** Merge (squash) only when all of these hold:
   - Sol approved the commits that are on the pull request now.
   - The builder's proof covers the change, including local test runs.
   - Every CI check is green on the head commit.
   - GitHub reports the pull request mergeable, with no conflicts against `main`.
   - The diff stays inside the slice's scope, the task ids are ticked in `docs/TODO.md`, and no unjustified dependency was added.
   - No decision number is used twice in `docs/DECISIONS.md`.
3. After merging, confirm GitHub reports it merged and that the `main` workflow run is green. Note any decision that later briefs must carry, then start the next slice.
4. If the gate fails (CI red, conflict with `main`, scope creep, missing proof): do not fix it yourself. Start a new builder on the same branch with a brief that states exactly what failed and what to do, and that it must rebase onto current `main`, re-run its tests, and get Sol's approval again if behaviour changed.
5. If a builder stopped without filing: read why. Then re-brief, split the slice, or, if the blocker is a genuine question for the owner, ask (§9).
6. If `main` is red after a merge: start a builder immediately with the single job of making `main` green, and merge nothing else in the affected crates until it lands.
7. If a builder's work contradicts a spec for a good reason, make sure the spec was updated in that pull request; if not, send a docs fix.

Every pull request that records a decision appends to `docs/DECISIONS.md`, so two waiting pull requests always conflict there. When that file is the only conflict, merge `main` into the branch yourself in a scratch worktree, keep both sides' entries in number order, check that the pull request's own diff is otherwise unchanged (`git diff origin/main HEAD` against the diff before), push, and wait for CI again; any other conflict goes to a rebase builder. Do not rebase such a branch: a union merge driver was tried (#66) and silently kept stale headings from the branch's intermediate commits, and GitHub ignores it.

Merge pull requests one at a time. After each merge, the next one in line must be rebased onto the new `main` and its checks must be green again before it merges; CI runs no tests, so two pull requests that each pass alone can still break each other. When the pull requests touch disjoint crates, a green rebase is enough. When they share code, have the second builder re-run the tests after rebasing.

Never merge around the gate, never use administrator overrides, and never fix a builder's code yourself.

## 8. Milestone reports

At the end of each milestone, check its exit criteria in `docs/ROADMAP.md` and post a short report in your thread:

- What is done and working, with the pull requests.
- Each exit criterion: met, not met, or cannot be checked by agents (say which).
- Measurements taken locally, labelled as such.
- Decisions made, and anything the owner should look at.
- What starts next.

Then continue to the next milestone without waiting, except after M4, where you stop.

## 9. Asking the owner

Ask only when a decision is genuinely the owner's and work cannot route around it: a spec ambiguity that changes the product, a need for credentials or paid services, a builder and reviewer deadlocked after the review budget, or anything destructive or outside the repository. Use the question tool, give the options with your recommendation first, and keep the other builders working meanwhile.

## 10. Standing rules

- Quality gates, test policy and CI policy are in `AGENTS.md` and are binding on every builder.
- CI stays small: format, lint, type-check and build checks only. Do not let a builder add audit, coverage or benchmark jobs.
- Test suites are not in CI. Every builder runs the relevant tests locally before filing and again after rebasing; say so in each brief.
- System-level tools come from `flake.nix`. A builder that needs a new one adds it to the flake in the same pull request instead of installing it by hand, and builders may run their commands through `nix develop -c` to get the pinned versions.
- Remind every builder of the dependency, logging and no-panic rules in `AGENTS.md`. Reject a slice report that adds a small convenience dependency without justification; send it back.
- `SRV-025` (the `cavea-telemetry` crate) lands before or with the first server endpoints, so that every endpoint is traced from the start.
- No secrets in briefs, commits or pull requests.
- Commit and pull-request attribution follows the harness's instructions.

## 11. Slice plan and open notes

The orchestrator keeps this section current so that the plan survives a context summary. Done work is whatever `docs/TODO.md` ticks on `main`; this lists only what is planned and what to remember. Last refreshed after pull request #99 (2026-11-12), at the end of M4.

**Where the work stands.** M1 to M4 are built as far as agents can take them; the milestone reports for M1, M2, M3 and M4 were posted in the orchestrator's thread. The owner's instruction is to stop after M4: do not start M5 or later without the owner saying so. No builder is running and no pull request is open, apart from any follow-up named below.

**Open, and whose it is:**

- **The owner's, before a first release:** a container registry and the `@cavea` npm scope (the names live in `apps/docs/site.json`); `cargo xtask release prepare <version>` with the changelog section, then a signed `v*` tag; a GitHub environment named `release`. Optional and later: the licence signing key (generated offline; its public half goes into `TRUSTED` in `crates/cavea-licence/src/keys.rs`, then `cargo xtask types`), a host and domain for the docs site, a lawyer's review of `LICENSE` (`VAL-020`).
- **The owner's, by nature:** reference-device runs (`TST-016`), reference-hardware load runs (`TST-024`), usability sessions (`TST-032`), the manual accessibility matrix, every `VAL` task, `OPS-004`, `OPS-005`.
- **Waiting for a later milestone's server work:** `DSN-040` and `CON-006` (a version diff, `SRV-036`), `CON-007` (two event settings), `SEC-002` (webhooks), `SRV-039` (the manager stream), the renderer's manager mode (`CON-020`).
- **Waiting for hardware:** `DSN-044` (a GPU measurement), running the arm64 image.
- **Small, agent-doable, not started:** `DSN-034` (grouping: needs a chart-schema change the owner has not decided; 07 §5.1 gives the two designs, and the orchestrator recommended a `group` id on decoration nodes), making "Powered by Cavea" a link once there is a domain. `DSN-061` and `DSN-062` were done in pull request #99 (D-99).

**How the work was run, for whoever continues it.** Up to three builders ran side by side, each on one chain of slices that edit the same files (renderer and SDK; designer; server; console; docs). When a builder split a slice, the next builder in that chain inherited the unreviewed branch. The lessons that cost time (§5 and §7 have the first two in full): a builder's final report sometimes never arrives (read the task's status); a builder can be cut off (rescue its worktree's uncommitted work to its branch, then start a continuation with the original brief); the secret scanner fails pull requests on test text that merely looks like a credential (briefs say how to write such tests); `bun test` scans scratch copies under `target/`; a long local run must be a `systemd-run --user` service, not a child of the session's shell; the owner's rule that breaking changes are free until a customer is live.

Notes for milestone reports and later briefs:

- **Payload.** Production payload, brotli, measured locally: 37,298 B (first slice), 44,403, 57,277, 60,852, 59,175, 70,337, 79,638, 84,301, 98,593 (glyph atlas, 9.5 KB), 104,057 (the SDK bundle is measured from here on), 109,658 (server adapter). The gate (D-28) fails a build more than 3% over `xtask/payload-baseline.json` or over 200,000 B; target 150 KB. About 40 KB is left for the second text slice, the built-in interface, accessibility and fallbacks.
- **Proof.** All browser proof is software rendering in headless Chromium. Nothing has run on a hardware GPU, a second browser engine or a phone. Reference-device budgets and reference-hardware load runs cannot be checked by agents.
- **Decisions the owner may want to revisit** (all recorded in `docs/DECISIONS.md` or the specs): D-23 one-statement commit; D-24 server starts without an admin key; D-25 decoration carries its floor, strokes built without `lyon`; D-26 public chart URL is the content hash with no credential, gzip dropped; D-27 snapshot re-encoded only with a delta; D-31 realtime routes take the publishable key in `?key=`; D-33 glyph atlas choices (notches at device pixel ratio 3); D-34 session tokens are signed with the SHA-256 of a secret key; D-35 what "development" means in the SDK; D-37 the token hash is not logged; an expired hold token is `404`, not `410`; a wrong HTTP method is `404`; hold-token creation is not idempotent; public routes ignore `Idempotency-Key`; a buyer's hold token never travels in a URL; buyers cannot extend a hold; `sqlx` and WebSocket support took the server from 58 to 194 crates.
- **For the owner's go-to-market list:** the Roboto Condensed licence notice (SIL OFL 1.1) must ship with the renderer. The 24-hour fuzz run of `ccf_decode` has not been made (30 minutes per target, no findings); the command is in `AGENTS.md`.
- **Gaps carried forward:** `mailbox_wait` and `apply` spans emit nothing on success (span-close events are off in `cavea-telemetry`), so per-stage timing is not in logs; the snapshot route's 99th percentile was 25.6 ms during a connection burst in a small load run, against a 3 ms budget (cause not yet found); `session: 'none'` still selects locally in the SDK; the `hold.expired` webhook row is not written (M5); `banquet` and `festival` presets, E004, tables, booths and areas wait for M5. From the earlier list and not rechecked since: no admin call mints a key for a workspace whose keys are all lost; the server formats findings itself instead of using the compiler's JSON form; a seat override keyed by an `oid` outside its row is not reported.
- **Measured locally (Ryzen 9 7900X, PostgreSQL 18 in Docker on loopback), for the M1 and M2 reports:** compile 100,000 seats about 65 ms; CCF decode 0.53 ms per 100,000 seats; hit test 0.30 µs; snapshot apply 0.17 ms; 10,000-change delta 0.19 ms; label build 0.16 ms per 4,096; compiled sizes, brotli: theatre 6.6 KB, arena 53.8 KB, stadium-100k 181.2 KB; one event through the actor against PostgreSQL 45,700 commands per second; load 100,000 objects 35 ms; hold over HTTP p50 2.0 ms, p99 3.7 ms; public hold p50 2.2 ms, p99 5.3 ms; publish 100,000 seats 576 ms; delta delivery p95 91 ms; snapshot request p99 0.43 ms unloaded; about 11.2 KiB per idle WebSocket with 10,000 open; hold in one browser to visible in a second, median 1 ms, p95 3 ms after the 100 ms coalescing interval; simulation 10,000,000 steps in 36.2 s with every invariant held and 18 planted bugs caught; reference model 40,000,000 steps without a defect; reconciler about 1,000,000 log rows per second; fuzzing 30 minutes per target without a finding.
