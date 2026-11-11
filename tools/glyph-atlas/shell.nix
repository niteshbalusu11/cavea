# The tools that bake the renderer's glyph atlas (docs/04-renderer.md §12),
# pinned so that a rebake is byte for byte the same. Only the person who
# rebakes needs this; CI and builders use the committed output. Run from the
# repository root:
#
#   nix-shell tools/glyph-atlas --run 'python3 -I tools/glyph-atlas/bake.py'
#
# The first run builds msdf-atlas-gen from source (about a minute).
let
  # The nixpkgs revision of flake.nix, pinned separately so that a flake
  # update does not change the atlas.
  pkgs =
    import
      (builtins.fetchTarball {
        url = "https://github.com/NixOS/nixpkgs/archive/02c087f3fe673c9e317f206a627cb6b910336dbe.tar.gz";
        sha256 = "1l60ar0j7j7jrrg4r7j4kscrih0qn2n0lslffzflhv86lyq650r2";
      })
      { };
  # Viktor Chlumsky's generator, which nixpkgs does not package.
  msdf-atlas-gen = pkgs.stdenv.mkDerivation {
    pname = "msdf-atlas-gen";
    version = "1.4";
    src = pkgs.fetchFromGitHub {
      owner = "Chlumsky";
      repo = "msdf-atlas-gen";
      rev = "v1.4";
      fetchSubmodules = true;
      hash = "sha256-kv1sZmzVktAn17dJdJEMC+YJ8oG6yDCKv8qn3TYUrD0=";
    };
    nativeBuildInputs = [
      pkgs.cmake
      pkgs.pkg-config
    ];
    buildInputs = [
      pkgs.freetype
      pkgs.tinyxml-2
      pkgs.libpng
    ];
    cmakeFlags = [
      "-DMSDF_ATLAS_USE_VCPKG=OFF"
      "-DMSDF_ATLAS_USE_SKIA=OFF"
      "-DMSDF_ATLAS_NO_ARTERY_FONT=ON"
      "-DMSDF_ATLAS_DYNAMIC_RUNTIME=ON"
    ];
    installPhase = "install -Dm755 bin/msdf-atlas-gen $out/bin/msdf-atlas-gen";
  };
in
pkgs.mkShell {
  packages = [
    msdf-atlas-gen
    pkgs.oxipng
    # fontTools removes the overlapping contours of Roboto 3's static fonts,
    # which otherwise leave artefacts in the distance field.
    (pkgs.python3.withPackages (ps: [
      ps.fonttools
      ps.skia-pathops
    ]))
  ];
  # Roboto 3.016 (SIL Open Font Licence 1.1).
  ROBOTO = "${pkgs.roboto}/share/fonts/truetype";
}
