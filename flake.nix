{
  description = "Rust-native tools for real-world robotics";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachSystem
      # nixos-unstable 26.11 no longer supports x86_64-darwin.
      [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ]
      (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          llvmPackages = pkgs.llvmPackages_latest;
          # Pin the complete Rust toolchain so development stays aligned.
          rustToolchain = pkgs.rust-bin.nightly."2026-06-29".default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "rustfmt"
              "clippy"
              "llvm-tools-preview"
            ];
            # The studio's web interface compiles to WebAssembly.
            targets = [ "wasm32-unknown-unknown" ];
          };
          # nixpkgs kept Tailwind 3 under the unsuffixed name for a while.
          tailwind = pkgs.tailwindcss_4 or pkgs.tailwindcss;
          # `dx` builds the web client and native server as one fullstack app,
          # compiles the declared Tailwind input, and bundles both. With
          # NO_DOWNLOADS set it takes every tool from PATH instead of fetching
          # its own, so the whole toolchain is pinned here.
          webBuildInputs = [
            pkgs.dioxus-cli # fullstack dx bundle / dx serve
            tailwind # compiles tailwind.css
            # Must match the `wasm-bindgen` version pinned in
            # crates/frontend/Cargo.toml; wasm-bindgen refuses a mismatch.
            pkgs.wasm-bindgen-cli_0_2_126
            pkgs.binaryen # wasm-opt, used for release builds
          ];
          nativeBuildInputs = [
            llvmPackages.libclang
          ];
          linuxBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.v4l-utils
          ];
          # Standalone driver archives are verified against Spiral's release
          # manifest. Rust builds never compile the embedded engine.
          adbcManifest = builtins.fromJSON (builtins.readFile ./drivers/spiral-adbc.json);
          adbcTarget = {
            aarch64-darwin = "aarch64-apple-darwin";
            aarch64-linux = "aarch64-unknown-linux-gnu";
            x86_64-linux = "x86_64-unknown-linux-gnu";
          }.${system};
          adbcArtifact = adbcManifest.artifacts.${adbcTarget};
          adbcLibrary = "libadbc_driver_spiraldb${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}";
          spiralAdbc = pkgs.stdenvNoCC.mkDerivation {
            pname = "spiral-adbc";
            version = adbcManifest.version;
            src = pkgs.fetchurl {
              inherit (adbcArtifact) url sha256;
            };
            nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.autoPatchelfHook ];
            buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.stdenv.cc.cc.lib ];
            dontBuild = true;
            installPhase = ''
              runHook preInstall
              mkdir -p "$out/lib"
              cp ${adbcLibrary} "$out/lib/"
              runHook postInstall
            '';
          };
          sharedShell = {
            packages =
              [
                rustToolchain
                pkgs.actionlint
                pkgs.pkg-config
                pkgs.python312
                pkgs.ruff
                tailwind
                pkgs.uv
                llvmPackages.llvm
              ]
              ++ nativeBuildInputs
              ++ linuxBuildInputs;
            # Preserve an explicitly selected validation build while the public
            # release manifest catches up with upstream transaction fixes.
            shellHook = ''
              export SPIRALDB_ADBC_DRIVER="''${SPIRALDB_ADBC_DRIVER:-${spiralAdbc}/lib/${adbcLibrary}}"
            '';
            ASAN_SYMBOLIZER_PATH = "${llvmPackages.llvm}/bin/llvm-symbolizer";
            LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
          };
        in
        {
          packages.spiral-adbc = spiralAdbc;

          devShells.default = pkgs.mkShell (sharedShell // {
            packages =
              sharedShell.packages
              ++ [
                pkgs.cargo-fuzz
                pkgs.cargo-binutils
                pkgs.ffmpeg
              ]
              ++ webBuildInputs;
            # Make `dx` use pinned tools rather than downloading its own copies
            # at build time.
            NO_DOWNLOADS = "1";
          });

          devShells.ci = pkgs.mkShell sharedShell;
        }
      );
}
