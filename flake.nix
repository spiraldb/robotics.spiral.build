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
            # The upstream PyTorch wheel is redistributable but marked unfree
            # in nixpkgs. Permit only that binary package so entering the shell
            # does not compile libtorch locally for hours.
            config.allowUnfreePredicate = pkg: nixpkgs.lib.getName pkg == "torch";
            # nixpkgs currently marks torch-bin broken globally when its CUDA
            # bindings lead the CUDA package set, even on macOS where CUDA is
            # not part of the wheel. Ignore only that macOS false positive and
            # retain the warning on Linux; CUDA deployments may still override
            # the worker interpreter explicitly.
            config.problems.handlers.torch.unsupported-cuda-version =
              if system == "aarch64-darwin" then "ignore" else "warn";
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
          # `dx` builds the web client and native server as one fullstack app,
          # compiles the declared Tailwind input, and bundles both. With
          # NO_DOWNLOADS set it takes every tool from PATH instead of fetching
          # its own, so the whole toolchain is pinned here.
          webBuildInputs = [
            pkgs.dioxus-cli # fullstack dx bundle / dx serve
            tailwind # compiles tailwind.css
            # Must match the `wasm-bindgen` version pinned in
            # crates/rerobot-studio/Cargo.toml; wasm-bindgen refuses a mismatch.
            pkgs.wasm-bindgen-cli_0_2_126
            pkgs.binaryen # wasm-opt, used for release builds
          ];
          linuxNativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            llvmPackages.libclang
          ];
          linuxBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.v4l-utils
          ];
          # The Studio owns orchestration in Rust. This interpreter is only the
          # isolated tensor/Arrow compute worker: PyTorch trains the model and
          # PyArrow decodes Rust-framed IPC batches. Spiral ADBC and checkpoint
          # publication remain entirely in the Rust backend.
          trainingPython = pkgs.python312.withPackages (ps: [
            ((ps."torch-bin" or ps.torch).overridePythonAttrs (_: {
              # The 2.12 wheel metadata caps setuptools below the version in
              # this nixpkgs revision even though setuptools is not imported
              # by the worker at runtime. Verify real imports below instead.
              dontCheckRuntimeDeps = true;
            }))
            ps.pyarrow
            ps.numpy
          ]);
        in
        {
          packages.training-python = trainingPython;

          devShells.default = pkgs.mkShell {
            packages =
              [
                rustToolchain
                pkgs.pkg-config
                pkgs.cargo-fuzz
                llvmPackages.llvm
                pkgs.cargo-binutils
                pkgs.ffmpeg
                trainingPython
              ]
              ++ webBuildInputs
              ++ linuxNativeBuildInputs
              ++ linuxBuildInputs;

            # Make `dx` use the tools declared above rather than downloading
            # its own copies at build time.
            NO_DOWNLOADS = "1";

            # Keep the Python boundary reproducible and invisible to Rust
            # callers. An explicit --training-python still wins when testing a
            # CUDA-specific environment outside nixpkgs.
            PYTHONNOUSERSITE = "1";
            REROBOT_TRAINING_PYTHON = "${trainingPython}/bin/python";

            ASAN_SYMBOLIZER_PATH = "${llvmPackages.llvm}/bin/llvm-symbolizer";
            LIBCLANG_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux "${llvmPackages.libclang.lib}/lib";
          };
        }
      );
}
