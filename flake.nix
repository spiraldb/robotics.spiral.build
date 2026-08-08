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
          };
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
              ++ linuxNativeBuildInputs
              ++ linuxBuildInputs;

            ASAN_SYMBOLIZER_PATH = "${llvmPackages.llvm}/bin/llvm-symbolizer";
            LIBCLANG_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux "${llvmPackages.libclang.lib}/lib";
          };
        }
      );
}
