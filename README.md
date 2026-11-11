# Cavea

A seat-map toolkit for ticketing platforms: a buyer-facing seat picker for web and mobile, a chart designer, and a self-hosted server that holds per-event seat state.

**Status:** early development. Nothing here is usable yet. "Cavea" is a placeholder name.

- Specs: [`docs/README.md`](docs/README.md)
- Build order: [`docs/ROADMAP.md`](docs/ROADMAP.md)
- Task list: [`docs/TODO.md`](docs/TODO.md)
- Instructions for coding agents: [`AGENTS.md`](AGENTS.md)

## Development

You need Nix (with flakes) and Docker for PostgreSQL. `flake.nix` provides the pinned Rust toolchain, bun, `wasm-opt`, `psql` and `brotli`: run `nix develop`, or `direnv allow` once to load it on `cd`, or prefix a single command with `nix develop -c`. Without Nix: Rust through rustup (it reads `rust-toolchain.toml`), bun at the version in `package.json`, and binaryen 132 for `wasm-opt` (the renderer's payload gate needs it).

```sh
cargo xtask build-web                               # WASM renderer into packages/renderer/wasm/
bun install
(cd apps/console && bun run dev)                   # console at the printed URL
cargo xtask build-web --demo && (cd apps/demo && bun run dev)  # 100,000 synthetic seats
docker compose -f deploy/compose.yaml up -d --wait  # PostgreSQL
cargo run -p cavea-server                           # GET /healthz on CAVEA_LISTEN (default 0.0.0.0:8080)
cargo run -p cavea-cli -- gen --list                # synthetic charts; also compile, validate, inspect
```

Checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `bun run lint`, `format:check`, `typecheck`, `build`, `test`. The full list is in [`AGENTS.md`](AGENTS.md#commands).

## Licence

No licence is granted. All rights reserved. The source is published for visibility only.
