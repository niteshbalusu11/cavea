# Orchestration playbook

**Status:** v1 · 2026-11-10

How this repository gets built: one orchestrator thread hands slices of `docs/TODO.md` to build agents, each of which ships its slice to `main` through a reviewed pull request. This file is the orchestrator's standing instructions. If your context is ever summarised, re-read this file, `AGENTS.md` and `docs/TODO.md`, then run `gh pr list` to rebuild your picture of what is in flight.

## 1. Roles

| Role | Model | Does | Does not |
|---|---|---|---|
| **Orchestrator** | Claude Fable 5.2, high effort | Chooses slices, writes briefs, starts builders, checks results, keeps `docs/TODO.md` honest, reports at milestone boundaries | Write product code, review code, merge a builder's pull request for it |
| **Builder** | Claude Opus 5.5, high effort, "Claude Work" provider | Implements one slice in its own worktree and ships it with the `full-send` skill | Start other builders; approve its own work |
| **Reviewer** | GPT-6.1 Sol, high reasoning effort | Reviews a builder's commits through the `sol-loop` skill, started by the builder | Edit code |

The orchestrator only orchestrates. If something needs a code change, however small, it goes to a builder. The orchestrator may commit documentation-only changes (ticking the task list, correcting a spec, this file) through a pull request of its own.

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
2. **Authority:** "Full send" — it must load and follow the `full-send` skill to a squash-merged pull request, and it is allowed to start `sol-loop` review delegates using GPT-6.1 Sol (`gpt-6.1-sol`, codex provider) at high reasoning effort, and no other subagents.
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

Slice D in detail: a `flake.nix` dev shell that provides every tool the repository needs beyond a bare OS (the Rust toolchain exactly as pinned in `rust-toolchain.toml` including the `wasm32-unknown-unknown` target, `wasm-bindgen-cli` at the version matching `Cargo.lock`, `binaryen` for `wasm-opt`, `brotli`, `bun`, a PostgreSQL client, and anything else the scaffold installed by hand), plus an `.envrc` containing `use flake`, so that `cd` into the repository followed by `direnv allow` loads everything. Commit `flake.lock`; ignore `.direnv/`. Proof: in a fresh shell, `nix develop -c` runs every command in the `AGENTS.md` "Commands" section successfully. CI does not use Nix; it keeps its plain cached toolchain setup, and the flake must stay in agreement with it.

## 6. Concurrency and the machine

- **At most three builders at once.** The machine has 24 cores and about 18 GB of free memory, and each builder caps cargo at 8 jobs.
- Drop to two if builds start failing for lack of memory or the machine thrashes.
- GitHub Actions is not the constraint (public repository); local compute is.
- Between waves, check for leftovers: stray containers (`docker ps`), dev servers, and disk used by old worktrees' build directories.

## 7. When a builder finishes

1. Read its final report.
2. Verify, do not trust: the pull request is merged (`gh pr view`), the `main` workflow run is green, and the task ids are ticked in `docs/TODO.md` on `main`.
3. If it merged: note any decision that later briefs must carry, then start the next slice.
4. If it stopped without merging: read why. Then either re-brief a builder with the missing information, split the slice, or, if the blocker is a genuine question for the owner, ask (§9).
5. If `main` is red after a merge: start a builder immediately with the single job of making `main` green, and start nothing else in the affected crates until it lands.
6. If a builder's work contradicts a spec for a good reason, make sure the spec was updated in that pull request; if not, send a docs fix.

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
