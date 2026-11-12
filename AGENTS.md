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

Run everything from the repository root. Prefix every cargo command with `CARGO_BUILD_JOBS=8` (shown once here, implied below).

Toolchains: Rust is pinned in `rust-toolchain.toml` (with the `wasm32-unknown-unknown` target); bun is pinned in `package.json` (`packageManager`). `flake.nix` provides every system tool at those versions (Rust with `rust-lld`, bun, `wasm-opt` from binaryen, `psql`, `brotli`): enter it with `nix develop` or `direnv allow`, or run one command as `nix develop -c <command>`. A new system tool goes into the flake, not onto the machine by hand. Docker is the only host requirement. Outside the flake, without `wasm-opt`, `build-web` still writes the bindings but the payload gate fails: unoptimised sizes are not comparable with the baseline.

**Rust**

- `CARGO_BUILD_JOBS=8 cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo xtask check-deps`: enforces the dependency rule in `docs/01-architecture.md` §3
- `cargo xtask build-web`: builds `cavea-web` for wasm32 with the `wasm-release` profile, writes the bindings to `packages/renderer/wasm/` (gitignored), runs `wasm-opt`, bundles the SDK's public entry with `bun build --minify`, prints raw and brotli sizes, and enforces the payload gate (`docs/13-testing-benchmarks.md` §6.3): the build fails when the SDK bundle plus WASM plus atlas is over 200,000 B brotli, more than 3% over the baseline in `xtask/payload-baseline.json`, built without `wasm-opt`, or built with a `wasm-opt` version other than the baseline's. A pull request that grows the payload on purpose runs `cargo xtask build-web --update-baseline` and commits the file, so the new size shows in the diff. `--demo` adds the debug-only exports `apps/demo` needs (synthetic seats); it is never gated, measured or shipped
- `cargo xtask build-designer`: builds `cavea-designer` (the designer's edit core as WebAssembly, `docs/07-designer.md` §2.4) the same way into `packages/designer-core/wasm/` (gitignored) and prints its raw and brotli sizes; no gate
- `cargo xtask build-server [--target <triple>]`: the server with the console compiled in (09 §9, D-62): runs `build-designer` and `build-web` (so `wasm-opt` is needed), `bun run build` in `apps/console` (after `bun install`), writes the console's files and their brotli forms with a table to `target/console/`, then `cargo build --release -p cavea-server` with `CAVEA_CONSOLE_EMBED=target/console/table.rs`, and prints the binary's size and what the console adds. Without `CAVEA_CONSOLE_EMBED` (every other build, CI) the server has no console and its pages say how to build one
- `cargo xtask types`: regenerates `packages/designer-core/src/generated/` (TypeScript types and a schema) from the Rust definitions (D-41), `packages/renderer/src/generated/engine.ts` (the events, object payload and option names the buyer WASM and `@cavea/renderer` exchange) from `crates/cavea-web/src/schema.rs` (D-71), and `apps/console/src/api/generated.ts` (the console's API types and operation table) from `crates/cavea-server/openapi.json` (D-60). Run it after changing a type that crosses the designer's boundary or the call table in `crates/cavea-designer/src/calls.rs`, after changing `schema.rs` (which `cargo test -p cavea-web` makes you do when an engine event, its JSON in `events.rs` or a theme or gesture option changes), and after regenerating the OpenAPI document; `cargo test -p xtask` and `cargo xtask types --check` fail while the committed files are stale. The generated files are not formatted by oxfmt
- `cargo run -p cavea-server -- openapi > crates/cavea-server/openapi.json`: regenerates the committed OpenAPI document (08 §6.1, D-46) after a change to a route or a schema; `cargo test -p cavea-server` fails while it is stale. The file is not formatted by oxfmt
- `cargo run --release -p cavea-cli -- <command>` (binary `cavea`): `gen <preset> [--seed n] [-o file]` writes a synthetic preset's chart document (`gen --list` lists them), `compile [doc.json] [-o dir]` writes the public and full artefacts named by content hash, `validate [doc.json] [--json]` prints findings, `inspect <chart.ccf>` prints counts and chunk sizes. A missing path reads standard input: `cavea gen theatre | cavea compile`. `bench [name…] [--json] [--save file] [--baseline file]` runs the micro-benchmarks (release builds only). Exit codes: 0 ok, 1 invalid input, 2 usage, 3 other failure, 4 a benchmark over its budget or more than 10% slower than the baseline. Changing a preset or the compiler's output changes the preset hashes pinned in `crates/cavea-testkit/src/presets/tests.rs`; re-measure the sizes in `docs/03-chart-format.md` §4.5 when they change
- `cargo test -p cavea-sim` runs the engine's model-based tests and the saved cases in `crates/cavea-sim/cases/` (13 §3.1). Before filing a change to `cavea-engine`, also run `cargo run --release -p cavea-sim --example soak -- model --seeds 1..20001 --steps 400 --jobs 4` and `… --example soak -- sim --seeds 1..2001 --steps 1000 --jobs 4` (seconds each; the M2 criterion is the second at 1,000,000 steps or more); a model failure prints a shrunk case to save there, a simulation failure the seed and step
- `cargo xtask determinism`: runs the tests that pin compiled hashes (the compiler's and the presets') natively and for wasm32 under bun; fails if either leg fails or cannot run. Run it before filing any change to the compiler, `cavea-format`'s writer or the presets
- `cargo xtask golden [--update] [--runs n] [case…]`: the renderer's golden images (`docs/13-testing-benchmarks.md` §6.1, D-66): builds the release WASM and the CLI, draws `crates/cavea-render/golden/cases.json` in the flake's headless Chromium (`nix build .#chromium .#golden-fonts`, fetched on first use) and compares each with its stored PNG; a failure writes the drawn and difference images to `target/golden/actual/`. About 10 s once built. `--update` redraws baselines (review the image diff and say why in the pull request); `--runs 3` checks three browser starts draw the same bytes. It overwrites `packages/renderer/wasm/` with the release build (run `build-web --demo` again for the demo)
- `cargo test -p cavea-web traces`: the interaction traces (13 §6.1): every `bench/traces/*.json` replayed through the engine must produce its `.events` file; `CAVEA_BLESS=1` rewrites them (review the diff). Part of `cargo test --workspace`. Run both commands before filing a change to `cavea-render`, `cavea-web`, the shaders or the SDK's host or text code
- `cargo run --release -p cavea-cli -- bench --save target/bench.json` on `main`, then `… bench --baseline target/bench.json` on the branch: run before filing a change to decoding, hit testing, state application, labels or the compiler; more than 10% slower fails. The baseline is this machine's only and is never committed (`docs/13-testing-benchmarks.md` §6.3)
- Fuzzing (`docs/13-testing-benchmarks.md` §5) uses a second dev shell with a nightly toolchain and `cargo-fuzz`, for the separate crate in `fuzz/` (not a workspace member): `nix develop .#fuzz -c cargo fuzz run <target> -- -max_total_time=<seconds> -timeout=10`, with `<target>` one of `ccf_decode`, `document_parse`, `wire_decode`, `snapshot_apply`. Seeds are `fuzz/corpus/<target>/seed-*` (regenerate with `nix develop .#fuzz -c cargo run --manifest-path fuzz/Cargo.toml --example seeds`); the grown corpus and `fuzz/artifacts/` are gitignored. Use one core per target and stop runs you start
- `CAVEA_DATABASE_URL=postgres://cavea:cavea@127.0.0.1:5432/cavea CAVEA_LISTEN=127.0.0.1:<port> cargo run -p cavea-server`: the server; it migrates the database at startup and refuses to start without `CAVEA_DATABASE_URL`. `GET /healthz` answers `ok`; `GET /readyz` answers `ready`, or `503` while the database is unreachable or the server is stopping; `GET /metrics` (with `Authorization: Bearer $CAVEA_ADMIN_KEY`) is Prometheus text (08 §15). Realtime streams run on a second runtime whose threads are `cavea-realtime` (D-45). Settings are `CAVEA_` environment variables (`docs/08-server.md` §13); add `CAVEA_LOG_FORMAT=text` for readable logs
- The console owner, locally: while the database has no user, the server logs `setup_link="/console/setup#token=<64 hex>"` at start (prefixed by `CAVEA_PUBLIC_URL` when set; a restart makes a new one, valid an hour). Without the console's screen, `curl -X POST -H 'Content-Type: application/json' -d '{"token":"<token>","email":"you@example.org","password":"<15 to 128 characters>"}' http://127.0.0.1:<port>/console/api/setup` creates the owner (owner of every workspace; with none, a workspace `main`) and returns the session cookie. Console settings: `CAVEA_CONSOLE_COOKIE_SECURE` (default `true`; `false` only for plain HTTP by a host name other than `localhost`), `CAVEA_RATE_LIMIT_SIGN_IN` (default 10 a minute per address). The console's dev server proxies `/console/api` and `/v1` to the server (09 §9)
- `CAVEA_DATABASE_URL=… cargo run --release -p cavea-server -- verify (--workspace KEY --event KEY | --event-id ID | --all) [--json]`: the reconciler (`docs/13-testing-benchmarks.md` §3.3); read-only, safe on a live database. Exit 0 clean, 1 differences, 2 usage, 3 could not run
- `cargo run --release -p cavea-loadgen -- --url http://127.0.0.1:<port> --chart <doc.json> [--profile l1|l2] [--subscribers N --buyers N | --rate R --duration-s S] [--json file] [--acks file]`: the load generator (`docs/13-testing-benchmarks.md` §7.1), with `CAVEA_ADMIN_KEY` set; follow it with `cavea-server verify`; `--acks` writes every acknowledgement's `seq`, `op` and `oid`s to compare with `event_log` after a fault (13 §7.4). Exit 0 ok, 1 a correctness check failed, 2 usage, 3 could not run, 4 a budget missed. Keep local runs small and short on this shared machine

**TypeScript** (run `cargo xtask build-web` first; `typecheck`, `build` and `test` need the WASM output)

- `bun install` (CI uses `bun install --frozen-lockfile`)
- `bun run lint`: oxlint, warnings denied (`.oxlintrc.json`)
- `bun run format` / `bun run format:check`: oxfmt (`.oxfmtrc.json`; Markdown is not formatted). oxfmt also checks TOML files such as `Cargo.toml`, so run `bun run format:check` for every change, including Rust-only ones
- `bun run typecheck`: `tsc --noEmit` in every workspace package
- `bun run build`: builds every workspace package that has a `build` script. In `packages/renderer` and `packages/react` it writes `dist/`, the package as published (`docs/05-web-sdk.md` §1, D-58: `index.js`, the declarations, and for the renderer `cavea.min.js`, the WASM and the atlas); the workspace still imports their source. The renderer's build fails while `packages/renderer/api.md` (the public declarations as one file) is stale: a pull request that changes the public API runs `bun run build --update-api` in `packages/renderer` and commits the file, so the change shows in the diff
- `bun fixtures/run.ts [check…]`, after `cargo xtask build-web`: the compatibility checks (`docs/13-testing-benchmarks.md` §8): packs both packages, builds the fixtures in `fixtures/` (script tag, Vite, webpack, Rollup, esbuild, Next.js, React 18, three Content Security Policy set-ups) in `target/tmp/compat/` against the tarballs and loads each in headless Chromium; prints a table. On demand, never in CI; the first run needs the network; `csp-server` needs the compose PostgreSQL. The fixtures are not workspace members: never add them to the root `package.json`. Run it before filing a change to the packaging, the asset URLs or `caveaServer`'s transports
- `bun tools/changelog.ts [--check]`: the merged pull-request titles since the last release, which become `CHANGELOG.md`'s next section at release time (`docs/01-architecture.md` §6); `--check` fails when a commit on `main` since then is not a squash-merged pull request. Pull requests never edit `CHANGELOG.md`
- `bun run test`: `bun test`; the designer's boundary test also needs `cargo xtask build-designer` and runs `cavea gen` through cargo. `CAVEA_MEASURE=1 bun test packages/designer-core` adds its per-call timings
- `CAVEA_SERVER=http://127.0.0.1:<server port> bun run dev --port <port>` in `apps/console`: the console dev server, proxying `/console/api` and `/v1` to the server (09 §9); open it as `http://localhost:<port>` (the session cookie is `Secure`). The server's setup link works with the dev server's origin in front of its path. The designer is `/designer/local`, after `cargo xtask build-designer` (the WASM loads at run time, so `typecheck` and `build` pass without it): open a chart document from a file (`cavea gen theatre -o target/tmp/theatre.json`), by the menu or by dropping it on the window, and export it with Ctrl+S; a chart on the server opens from the charts list or its page at `/designer/<workspace key>/<chart key>`, saved as it is edited (07 §9). In a dev build `window.caveaEditor` is the open chart's editor (`apps/console/src/designer/editor.ts`) and, on a server chart, `window.caveaAutosave` its save machine (`autosave.ts`), for measuring in the browser's console. An event's page is `/events/<key>` (the manager; the console's `typecheck` and `build` need `cargo xtask build-web` first, for the renderer's bindings); in a dev build `window.caveaManager` has its `map`, its poller `live` (`live.reads()`: each whole read's requests, objects and time) and the renderer's `metrics`
- The console app's structure (`apps/console/src`): `index.tsx` (routes), `styles.css` (the design tokens; `tokens.test.ts` checks their contrast), `i18n/messages.ts` (every user-visible string), `ui/` (the base components), `api/` (the generated API types, `client.ts` the one request layer, `query.ts` server state for a screen, `permissions.ts`), `session.ts` (the signed-in person and the selected workspace), `console/` (the console frame and its screens; `console/manager/` the event page: `model.ts` the manager's rules, `live.ts` the polling state adapter, D-68), `designer/` (the designer: `loop.ts` the frame loop, `editor.ts` an open chart, `shortcuts.ts` the shortcut table, `CanvasHost.tsx` the canvas boundary)
- `bun run dev --port <port>` in `apps/demo`, after `cargo xtask build-web --demo`: 100,000 synthetic seats through `createSeatMap` and a `customState` fake host (query parameters in `apps/demo/src/main.ts`)
- The benchmark page (13 §6.2), after `cargo xtask build-web --demo`: `bun bench/collect.ts [--listen host:port]` (writes to the gitignored `bench/local-results/`), then `bun run build && bun run preview --port <port>` in `apps/demo` and open `bench.html?collector=http://127.0.0.1:<collector port>` (`auto=1` starts it, `runs=1` shortens it). Results from this machine are smoke runs: never commit them
- `bun packages/designer-core/proof/serve.ts --port <port>` (0 picks a free one; the URL is printed), after `cargo xtask build-designer`: the designer canvas's proof page, a test harness and not product interface. `?preset=<name>` opens a preset's draft (made with `cavea gen` through cargo), `theme=dark`, `bar=0` hides the buttons, `camera=x:y:width:height` frames a box as the demo's parameter does (so the two pages can be compared); wheel zooms, the hand tool or the middle button pans, input goes to the session, and `window.proof` has the session, the canvas, the view and the draws counted. Served by Bun's own bundler, so `bun run build` never builds it
- `cargo test --release -p cavea-designer measure_canvas -- --ignored --nocapture`: the canvas against the designer targets on the recording backend (first draw, a block's pitch change, drag steps, baking, the overlay)

**Image and deployment** (08 §14, D-63)

- `docker buildx build -f deploy/Dockerfile -t cavea-server .` (add `--platform linux/arm64` for arm64; cross-compiled, no emulation; `--output type=local,dest=<dir>` to inspect it without loading): the distroless image with the console. Pass `--build-arg CARGO_BUILD_JOBS=8` (the default). Tool versions in it must match `rust-toolchain.toml`, `package.json` and CI's binaryen; the build fails when bun's does not. Its BuildKit cache (cargo registry, target, bun) is kept between builds
- `nix develop -c bun deploy/smoke.ts [--image <tag>]`: builds the image (unless given), runs it with its own PostgreSQL under a unique compose project and a free port, and checks it end to end: setup link, owner, sign-in, chart, event, keys, hold and book, the console's files and headers, the reconciler, a clean stop, the data after a restart, no secret in the log. It removes every container, volume and network it made, and the image it built. Run it before filing a change to the Dockerfile, the compose files, start-up or shutdown
- `deploy/compose.server.yaml`: the production-shaped reference (the image and its own PostgreSQL; `POSTGRES_PASSWORD` required, `CAVEA_ADMIN_KEY` and `CAVEA_PUBLIC_URL` passed through when set). Never confuse it with `deploy/compose.yaml`, the shared development database; run your own copies only under a `-p` project name of your own

**Glyph atlas**

- `nix-shell tools/glyph-atlas --run 'python3 -I tools/glyph-atlas/bake.py'`: rebakes the seat labels' atlas (`packages/renderer/assets/glyphs.png`) and its metrics (`crates/cavea-render/src/text/glyphs.rs`) from the pinned font and tools (`docs/04-renderer.md` §12). The output is committed and deterministic; only a change to the glyph set or the font needs it. A clean `git status` afterwards verifies the committed files. The first run builds `msdf-atlas-gen` (about a minute)

**Database**

- `docker compose -f deploy/compose.yaml up -d --wait`: PostgreSQL 18 on `127.0.0.1:${CAVEA_PG_PORT:-5432}`, user, password and database all `cavea`
- `docker compose -f deploy/compose.yaml down`
- `CAVEA_TEST_DATABASE_URL=postgres://cavea:cavea@127.0.0.1:5432/cavea cargo test -p cavea-server -- --include-ignored --skip measure_`: the database tests (migrations, `PgStore`, the actor on PostgreSQL, keys, workspaces and tenant isolation through the router). Each creates and drops its own database. Without the variable `cargo test` lists them as ignored
- The same with `--release` and `measure_ -- --ignored --nocapture --test-threads=1`: prints commit latency, actor throughput and load time; `measure_console_reads` prints the staff state, counts and events list at 100,000 seats and 1,000 events. `cargo test --release -p cavea-engine --lib -- --ignored --nocapture measure_publication` prints what a publication costs the actor at 100,000 objects
- A schema change is a new file in `crates/cavea-server/migrations/`, listed in `crates/cavea-server/src/store/migrations.rs`; never edit a released one

CI (`.github/workflows/ci.yml`) runs the Rust checks, `check-deps` and `build-web` (with binaryen 132 installed, so the payload gate runs), then the TypeScript `lint`, `format:check`, `typecheck` and `build`. It runs no tests.

## Quality gates

Decently strict, not pedantic.

**Rust**

- `cargo fmt` clean.
- `cargo clippy` with warnings denied. Lints are configured once in the workspace `Cargo.toml`: the default clippy set, plus `dbg_macro`, `todo`, `unimplemented`, and `unwrap_used` and `expect_used` outside tests. Do not enable `pedantic` or `nursery` wholesale.
- `unsafe` only where an FFI or graphics API requires it, with a comment saying why it is sound.
- No panics in normal operation. Do not call `unwrap` or `expect` outside tests; propagate with `?` and return a `Result`. Indexing that can go out of bounds uses `get`.
- Library crates define their own error types and return them. Binaries (`cavea-server`'s `main`, `cavea-cli`, `xtask`) may use `anyhow` for top-level error handling with context.
- `cavea-format` treats every input byte as hostile.

**TypeScript**

- `strict: true`, plus `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `noImplicitOverride`, `noFallthroughCasesInSwitch`.
- oxc lint clean with no blanket disables. A single-line disable needs a reason.
- No `any` in exported types.

## Dependencies

Keep them few. Rust build time is paid on every change, locally and in CI, and the server is where it hurts most.

- Add a dependency only when it does substantial, hard-to-get-right work: the async runtime, the HTTP framework, the database driver, serialisation, cryptography, graphics bindings.
- Do not add a crate for something a few dozen lines of our own code can do: small helpers, trivial macros, one-function utilities, convenience wrappers.
- Prefer what the standard library and the crates already in the workspace provide.
- Turn off default features and enable only what is used.
- Every new dependency is named in the pull request with one line on why it is needed and what it adds to the build (`cargo tree` before and after). Avoid pulling in a second version of a crate already in the tree.
- The same restraint applies to JavaScript packages.

## Logging

Structured, never ad hoc.

- All logging goes through one workspace crate, `cavea-telemetry`, built on `tracing`. It owns setup (JSON output in production, readable output in development, level from configuration) and the HTTP layer.
- Every HTTP request gets a span with a request id, method, matched route, status and duration; the request id is returned in the `Cavea-Request-Id` header. Work done for a request (mailbox wait, apply, commit, database calls) happens inside child spans, so one request can be followed end to end.
- Log events carry fields, not interpolated strings: `info!(event = %key, seq, "hold committed")`.
- No `println!`, `eprintln!` or `dbg!` outside `xtask` and CLI output.
- Panics are logged by the hook `cavea_telemetry::init` installs (`log_panic`): one `error` record in the current span, with the location; a formatted message is withheld, since it may quote input. In the server a handler's panic answers `500` (08 §16).
- Never log secrets: API keys, hold tokens, session tokens, request bodies (`docs/12-security-licensing.md` §4). A secret in a typed request or response body (a password, a token, a new key, a link holding a token) is `http::extract::Secret`, whose `Debug` prints nothing; `secrets_never_reach_the_logs` drives every route in `ROUTES` and fails if a recognisable secret reaches any log record.
- In a handler: register the route through the route table, `Routes::add(Op::…, handler)` in its module's `routes()` (merged before `finish`, which adds the request and panic layers): the `Op` gives the router its method and path and the OpenAPI document its credential (`.needs(..)`), parameters, body and responses, by the schema names of `http/openapi/schemas.rs`; regenerate `openapi.json`. The router tests validate every response against the document, so a test of the route fails until it is right; return `Result<_, ApiError>` (`ApiError::new(Code::…)`, `.with_detail(..)`, `?` on an engine `Failure`); take bodies with `http::Json`, not `axum::Json`, so rejections are problem+json (wrap other extractors the same way); call `cavea_telemetry::record_workspace(..)` and `record_event(..)` once known; wrap database calls with `.instrument(cavea_telemetry::db_span("name"))`. Span names are in `cavea_telemetry::span` (`docs/08-server.md` §15).
- Credentials (`docs/08-server.md` §11): declare one extractor from `cavea_server::http`: `Staff(auth)` (a secret key, or a console session of a member whose role allows the route), `PublishableKey(auth)` or `AdminKeyAuth`; `/console/api` routes take `staff::ConsoleUser` (the signed-in user), or `Member(auth)` for a workspace's console-only routes (no API key; `.needs(Needs::Member(Permission::…))`). It answers `401`/`403` and records the workspace. A `Staff` route declares its permission in the route table, `.needs(Needs::Staff(Permission::…))` (09 §2's table is `console::Role::allows`, one permission per cell): there is no route a secret key may call without one, the matrix checks it against every role, the document shows it, and a new route adds its line to `PERMISSIONS` in the matrix's file (`every_route_declares_its_permission`). Scope every query by `auth.workspace().uuid()` (bind as `$n::uuid`); never take a workspace from the request alone. Pass `auth.authority()` and `auth.actor()` to the engine (`key_…` or `usr_…`); `auth.key_id()` and `auth.user()` say which. A cookie-authenticated non-safe request is checked for its CSRF token and origin by the extractor, so a `GET` must never change anything. Take `http::Path` and `http::Query` (problem+json rejections); paginate lists with `extract::Page` and `List`. A route needing more than the 256 KiB default body adds `.layer(DefaultBodyLimit::max(n))` to its method router.
- Security headers (Content Security Policy, `nosniff`, HSTS and the rest) come from one layer by path (`http/headers.rs`, 08 §6.5): never set them in a handler, and a new path prefix decides its family there. CORS for `/v1/public/` stays `public::cors`.
- Every new endpoint adds a row to `ROUTES` in `crates/cavea-server/src/http/tests/api.rs` (the authorisation matrix and tenant isolation test, 13 §10, 12 §3 T5), with a placeholder and fixture in `Tenant` if its path names an identifier; `every_route_has_a_row` fails until it does.

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
- The pull request title is the changelog entry (`docs/01-architecture.md` §6): one plain sentence on what changed for someone using Cavea, then the `docs/TODO.md` task ids in parentheses. Tick those items in the same pull request.
- Follow the `full-stop` skill: build, prove locally, review with `sol-loop`, rebase, file the pull request. Builders do not wait on CI and do not merge; the orchestrator runs the merge gate and merges (`docs/ORCHESTRATION.md` §7).
- Review is by GPT-6.1 Sol at high reasoning effort, through `sol-loop`. The builder does not approve its own work.
- Squash-merge. Rebase onto `main` before merging; never force-push `main`.
- Push your branch as soon as it has a commit, and again after every review round and rebase: a worktree is removed when its session ends, and work that exists only there is lost with it.
- Never end your turn while a command you started is still running: run long commands in the foreground, or wait for them in the same turn. A session that stops to wait may never be woken.
- Make the smallest change that completes the task. No unrelated cleanup.
- If code and spec disagree, fix one of them in the same pull request. A reversed design decision gets a new entry in `docs/DECISIONS.md`.
- A new decision takes the next free number after the ones on `origin/main` and the ones open pull requests claim (`gh pr list`, then each one's title and body). After a rebase, check that no number appears twice (`grep '^## D-' docs/DECISIONS.md | cut -d' ' -f2 | sort | uniq -d` prints nothing) and renumber your own entry, with its references, if one does.
- Check the payload effect of any new dependency in `cavea-core`, `cavea-format` or `cavea-web`.

## Local machine limits

Several agents share this machine.

- Set `CARGO_BUILD_JOBS=8` for every cargo invocation.
- Use the one shared PostgreSQL container from `deploy/compose.yaml`. Tests create and drop their own uniquely named database; never rely on a fixed database name or leave data behind.
- Do not leave servers, watchers or containers of your own running when you finish.
- Pick a free port at run time; do not hard-code one.
- `/tmp` is a 15 GB memory-backed filesystem shared by every agent, and it has filled up. Set `TMPDIR` to a gitignored directory in your worktree (`target/tmp`) for builds and tests, keep scratch files and review checkouts there, and delete your temporary files when you finish. Never delete another session's.

## Out of scope for agents

Tasks prefixed `VAL`, and anything needing a person (outreach, legal review, buying devices, user sessions), belong to the repository owner. Skip them.
