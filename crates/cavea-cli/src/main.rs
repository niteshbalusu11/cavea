//! The `cavea` command-line tool for developer machines and CI: compile,
//! validate and inspect charts, generate synthetic charts, and run benchmarks.
//! See `docs/01-architecture.md` §2.

fn main() {
    println!("cavea {}", env!("CARGO_PKG_VERSION"));
}
