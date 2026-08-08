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
          rustToolchain = pkgs.rust-bin.nightly."2026-05-17".default.override {
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
          # `dx` builds crates/rerobot-ui: it compiles the crate to wasm, runs
          # the Tailwind CLI on tailwind.css, and bundles the result. With
          # NO_DOWNLOADS set it takes every one of those tools from PATH
          # instead of fetching its own, so the whole toolchain is pinned here.
          webBuildInputs = [
            pkgs.dioxus-cli # dx build / dx serve
            tailwind # compiles tailwind.css
            # Must match the `wasm-bindgen` version pinned in
            # crates/rerobot-ui/Cargo.toml; wasm-bindgen refuses a mismatch.
            pkgs.wasm-bindgen-cli_0_2_126
            pkgs.binaryen # wasm-opt, used for release builds
          ];
          linuxNativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            llvmPackages.libclang
          ];
          linuxBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.v4l-utils
          ];
        in
        {
          devShells.default = pkgs.mkShell {
            packages =
              [
                rustToolchain
                pkgs.pkg-config
                pkgs.cargo-fuzz
                llvmPackages.llvm
                pkgs.cargo-binutils
              ]
              ++ webBuildInputs
              ++ linuxNativeBuildInputs
              ++ linuxBuildInputs;

            # Make `dx` use the tools declared above rather than downloading
            # its own copies at build time.
            NO_DOWNLOADS = "1";

            ASAN_SYMBOLIZER_PATH = "${llvmPackages.llvm}/bin/llvm-symbolizer";
            LIBCLANG_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux "${llvmPackages.libclang.lib}/lib";
          };
        }
      );
}
