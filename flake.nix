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
          rust = (rust-overlay.lib.mkRustBin { } pkgs).fromRustupToolchainFile ./rust-toolchain.toml;
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
        }
      );
    };
}
