# Cavea — instructions for agents

Cavea is a seat-map toolkit for ticketing platforms: a buyer-facing seat picker, a chart designer, and a self-hosted server that holds per-event seat state. "Cavea" is a placeholder name.

The specs in `docs/` are the source of truth. Start at `docs/README.md`. The task list is `docs/TODO.md`; the build order is `docs/ROADMAP.md`.

## Tech stack

| Layer | Choice |
|---|---|
| Shared core, compiler, renderer engine, server | Rust (edition 2024), one cargo workspace under `crates/` |
| Web renderer | Rust compiled to WebAssembly, drawing with WebGL2 through `glow` |
| Server | tokio, axum, sqlx, PostgreSQL 15+ |
| Web packages and apps | TypeScript, SolidJS, Vite, under `packages/` and `apps/` |
| JavaScript package manager and script runner | bun (never npm, pnpm or yarn) |
| TypeScript lint and format | oxc (`oxlint`, and the oxc formatter) |
| Type checking | `tsc --noEmit` in strict mode |
| Local services | Docker Compose (`deploy/compose.yaml`) |

Layout and dependency rules are in `docs/01-architecture.md` §3. `cavea-core` and `cavea-format` depend on nothing else in the workspace.

## Commands

The scaffold task fills this section in with the exact commands. Until then:

- Rust: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- TypeScript: `bun install`, then the `lint`, `format:check`, `typecheck`, `build` and `test` scripts in the root `package.json`
- Database: `docker compose -f deploy/compose.yaml up -d`

## Quality gates

Decently strict, not pedantic.

**Rust**

- `cargo fmt` clean.
- `cargo clippy` with warnings denied. Lints are configured once in the workspace `Cargo.toml`: the default clippy set, plus `dbg_macro`, `todo`, `unimplemented`, and `unwrap_used` and `expect_used` outside tests. Do not enable `pedantic` or `nursery` wholesale.
- `unsafe` only where an FFI or graphics API requires it, with a comment saying why it is sound.
- Library crates return typed errors and do not panic on bad input. `cavea-format` treats every input byte as hostile.

**TypeScript**

- `strict: true`, plus `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `noImplicitOverride`, `noFallthroughCasesInSwitch`.
- oxc lint clean with no blanket disables. A single-line disable needs a reason.
- No `any` in exported types.

## Tests

A test must earn its place.

- Write a test when it guards behaviour that matters and could plausibly break: the seat state machine, selection rules, format round trips, protocol codecs, and the paths in `docs/13-testing-benchmarks.md`.
- Do not write tests that restate the implementation, exercise trivial getters or constructors, assert on mocks you just configured, or exist to raise a coverage number.
- Prefer one property test over twenty example tests where the spec states an invariant.
- When unsure whether a test has value, leave it out and say so in the pull request.

## Continuous integration

Keep it small and fast.

- CI runs format checks, lint checks, type checks and build checks. Nothing else: no audit, deny, coverage or benchmark jobs.
- Test suites are run locally by the agent as proof before a pull request is filed, and again after rebasing.
- Cache aggressively: the cargo registry and target directory keyed on `Cargo.lock`, the bun cache keyed on `bun.lock`. Cancel superseded runs with a concurrency group. Use path filters so a docs-only change builds nothing.
- Linux runners only.

## Workflow

- Work on a branch in your own worktree. One pull request per task slice.
- Name the `docs/TODO.md` task ids in the pull request title, and tick those items in the same pull request.
- Follow the `full-send` skill end to end: build, prove locally, review with `sol-loop`, file the pull request, fix CI and review findings, merge to `main`.
- Review is by GPT-6.1 Sol at high reasoning effort, through `sol-loop`. The builder does not approve its own work.
- Squash-merge. Rebase onto `main` before merging; never force-push `main`.
- Make the smallest change that completes the task. No unrelated cleanup.
- If code and spec disagree, fix one of them in the same pull request. A reversed design decision gets a new entry in `docs/DECISIONS.md`.
- Check the payload effect of any new dependency in `cavea-core`, `cavea-format` or `cavea-web`.

## Local machine limits

Several agents share this machine.

- Set `CARGO_BUILD_JOBS=8` for every cargo invocation.
- Use the one shared PostgreSQL container from `deploy/compose.yaml`. Tests create and drop their own uniquely named database; never rely on a fixed database name or leave data behind.
- Do not leave servers, watchers or containers of your own running when you finish.
- Pick a free port at run time; do not hard-code one.

## Out of scope for agents

Tasks prefixed `VAL`, and anything needing a person (outreach, legal review, buying devices, user sessions), belong to the repository owner. Skip them.
