{
  description = "Cavea development shell";

  inputs = {
    # Pinned to a nixos-unstable revision whose bun matches `packageManager` in
    # package.json (checked below). Bump the two together.
    nixpkgs.url = "github:NixOS/nixpkgs/02c087f3fe673c9e317f206a627cb6b910336dbe";
    # Official Rust toolchains, read from rust-toolchain.toml. Includes rust-lld,
    # the default linker for wasm32-unknown-unknown.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      packageJson = builtins.fromJSON (builtins.readFile ./package.json);
      bunVersion = nixpkgs.lib.removePrefix "bun@" packageJson.packageManager;
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          rustBin = rust-overlay.lib.mkRustBin { } pkgs;
          rust = rustBin.fromRustupToolchainFile ./rust-toolchain.toml;
          # Fuzzing only (docs/13-testing-benchmarks.md §5): libFuzzer
          # instrumentation and sanitizers need a nightly compiler. Pinned to
          # one date; bump it with the rust-overlay input.
          nightly = rustBin.nightly."2026-11-05".minimal;
        in
        assert pkgs.lib.assertMsg (
          pkgs.bun.version == bunVersion
        ) "flake.nix: nixpkgs has bun ${pkgs.bun.version}, package.json pins ${bunVersion}";
        {
          default = pkgs.mkShell {
            packages = [
              rust
              pkgs.bun
              pkgs.binaryen # wasm-opt, used by `cargo xtask build-web`
              pkgs.brotli
              pkgs.postgresql_18 # psql; the server itself runs in Docker
            ];
          };
          # `nix develop .#fuzz`: the nightly toolchain and cargo-fuzz for the
          # crate in fuzz/, which is not part of the main workspace.
          fuzz = pkgs.mkShell {
            packages = [
              nightly
              pkgs.cargo-fuzz
            ];
          };
        }
      );
    };
}
