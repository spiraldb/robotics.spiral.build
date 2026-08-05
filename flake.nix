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
          # Pin the complete Rust toolchain so builds and development stay aligned.
          rustToolchain = pkgs.rust-bin.nightly."2026-05-17".default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "rustfmt"
              "clippy"
              "llvm-tools-preview"
            ];
          };
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          linuxNativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            llvmPackages.libclang
          ];
          linuxBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.v4l-utils
          ];
          rerobot = rustPlatform.buildRustPackage {
            pname = "rerobot";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [ pkgs.pkg-config ] ++ linuxNativeBuildInputs;
            buildInputs = linuxBuildInputs;

            LIBCLANG_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux "${llvmPackages.libclang.lib}/lib";
          };
        in
        {
          packages = {
            inherit rerobot;
            default = rerobot;
          };

          apps = {
            rerobot-find-port = {
              type = "app";
              program = "${rerobot}/bin/rerobot-find-port";
              meta.description = "Identify a robot controller's serial port";
            };
            rerobot-find-cameras = {
              type = "app";
              program = "${rerobot}/bin/rerobot-find-cameras";
              meta.description = "Find native cameras and capture test images";
            };
          };

          checks.default = rerobot;

          devShells.default = pkgs.mkShell {
            inputsFrom = [ rerobot ];
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
