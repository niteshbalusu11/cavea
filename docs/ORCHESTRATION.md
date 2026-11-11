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
3. After merging, confirm GitHub reports it merged and that the `main` workflow run is green. Note any decision that later briefs must carry, then start the next slice.
4. If the gate fails (CI red, conflict with `main`, scope creep, missing proof): do not fix it yourself. Start a new builder on the same branch with a brief that states exactly what failed and what to do, and that it must rebase onto current `main`, re-run its tests, and get Sol's approval again if behaviour changed.
5. If a builder stopped without filing: read why. Then re-brief, split the slice, or, if the blocker is a genuine question for the owner, ask (§9).
6. If `main` is red after a merge: start a builder immediately with the single job of making `main` green, and merge nothing else in the affected crates until it lands.
7. If a builder's work contradicts a spec for a good reason, make sure the spec was updated in that pull request; if not, send a docs fix.

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

The orchestrator keeps this section current so that the plan survives a context summary. Done work is whatever `docs/TODO.md` ticks on `main`; this lists only what is planned and what to remember. Last refreshed after pull request #27 (2026-11-11).

Three chains run side by side, one builder each, because each chain's slices edit the same crates. When a builder splits a slice, the next builder in that chain inherits the unreviewed branch and takes it through review together with the chain's next tasks.

**Renderer chain** (`cavea-render`, `cavea-web`, `packages/renderer`, `apps/demo`); the longest chain in M1:

1. In flight: `mesh` program, level of detail, section counters and tint, pulse: `REN-019`, `REN-021`, `REN-022`, `REN-043` (the pulse inherited from `slice/render-pulse-crowd`).
2. Text: MSDF atlas and `glyph` program, seat labels, dynamic text atlas and `text` program, row and section labels, icons: `REN-024` to `REN-028`. Likely two slices.
3. Context loss, timing marks, headless render, golden images: `REN-040`, `REN-044`, `REN-045`, `REN-006`, `TST-010`.
4. Web SDK: `WEB-001` to `WEB-005` (loader, `createSeatMap`, adapters, callbacks, methods), then `WEB-006` to `WEB-011` (built-in interface), then `WEB-012` to `WEB-018`; server adapter `WEB-020` to `WEB-025` once the realtime transports are merged. `FND-007` (generated TypeScript types) goes with the first SDK slice.
5. Accessibility baseline `A11Y-001`, `A11Y-002`; benchmark page and tests `TST-011`, `TST-012`, `TST-015`.
6. A payload-reduction slice if the gate shows the renderer heading past 150 KB (see notes).
7. `REN-020` (background images) waits for asset upload (`SRV-010`).

**Format, compiler and tooling chain** (`cavea-compiler`, `cavea-format`, `cavea-cli`, `cavea-testkit`, `xtask`):

1. In flight: CLI, presets, compiled sizes, CI payload gate: `FMT-024` to `FMT-026`, `FND-005`.
2. Fuzz targets and determinism automation: `TST-013`, `TST-014`, `TST-027`.
3. Then this slot starts M3's edit core early, since it needs only the compiler: `cavea-edit` document arena, commands with inverses, undo and redo, spatial index: `DSN-001` to `DSN-003`, `DSN-007`, `TST-030`.

**Server chain** (`cavea-engine`, `cavea-server`):

1. In flight: realtime transports: `RT-003` to `RT-010`, `SRV-021`.
2. Session tokens and rate limiting: `SRV-016`, `SRV-018`.
3. Objects query, readiness and metrics, OpenAPI, the tracing gap below: `SRV-014`, `SRV-019`, `SRV-020`.
4. M2 tests: reference model and simulation (`TST-020`, `TST-021`), reconciler and load generator (`TST-022`, `TST-023`), load and chaos runs (`TST-024`, `TST-025`), authorisation matrix completion and fuzz (`TST-026`).
5. Docker image and compose (`SRV-023`), Node server SDK (`WEB-026`, after OpenAPI), assets (`SRV-010`).

M3 (designer) and M4 (console, licensing, security, docs, release) follow in `docs/TODO.md` order. The console and designer share `apps/console`; the designer's Rust side is `cavea-edit`.

Notes for milestone reports and later briefs:

- **Payload.** Production renderer, brotli, measured locally: 37,298 B (first slice), 44,403 (camera), 57,277 (chart loading), 60,852 (themes), 59,175 (reader slimmed), 70,337 (selection), 79,638 (holds and state). Budget 150 KB, hard cap 200 KB, with text and its atlas, accessibility and the SDK interface still to come. Known large items: label generation and the JSON event encoder, about 5 KB each before optimisation; the CCF reader, about 9 KB; wire decoders 2.4 KB.
- **Proof.** All browser proof is software rendering in headless Chromium. Nothing has run on a hardware GPU, a second browser engine or a phone. Reference-device budgets cannot be checked by agents.
- **Decisions the owner may want to revisit** (all recorded in `docs/DECISIONS.md` or the specs): D-23 one-statement commit; D-24 server starts without an admin key; D-25 decoration carries its floor, strokes built without `lyon`; D-26 public chart URL is the content hash with no credential, gzip dropped; D-27 snapshot re-encoded only with a delta; an expired hold token is `404`, not `410`; a wrong HTTP method is `404`; hold-token creation is not idempotent; public routes ignore `Idempotency-Key`; a buyer's hold token never travels in a URL on public routes; buyers cannot extend a hold; `sqlx` took the server from 58 to 165 crates.
- **Gaps carried forward:** `mailbox_wait` and `apply` spans emit nothing on success (span-close events are off in `cavea-telemetry`), so per-stage timing is not in logs; `CAVEA_HOLD_TTL` is not read; no admin call mints a key for a workspace whose keys are all lost; `CreateHold` log rows lack the token hash and object limit, which the reconciler needs; the `hold.expired` webhook row is not written (M5); the server still formats findings itself instead of using the compiler's JSON form; a seat override keyed by an `oid` outside its row is not reported; HELLO carries a zero chart hash until the transports slice fixes the chart provider; `banquet` and `festival` presets, E004, tables, booths and areas wait for M5.
- **Measured locally (Ryzen 9 7900X), for the M1 and M2 reports:** compile 100,000 seats about 65 ms; CCF decode 0.55 ms per 100,000 seats; hit test 0.55 µs; snapshot apply 0.04 ms; 10,000-change delta 0.11 ms; one event through the actor against PostgreSQL 45,700 commands per second; load 100,000 objects 35 ms; hold over HTTP p50 2.0 ms, p99 3.7 ms; publish 100,000 seats 576 ms (brotli 488 ms).
