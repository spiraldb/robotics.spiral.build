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
          cudaLicenseNames = [
            "CUDA EULA"
            "cuDNN EULA"
            "cuSPARSELt EULA"
          ];
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
            # The upstream PyTorch wheel and the CUDA runtime it links on Linux
            # are marked unfree in nixpkgs. Permit only that binary/runtime
            # closure so entering the shell does not compile libtorch locally
            # for hours.
            config.allowUnfreePredicate =
              pkg:
              builtins.elem (nixpkgs.lib.getName pkg) [
                "torch"
                "cuda-bindings"
                "triton"
              ]
              || nixpkgs.lib.all
                (license: license.free || builtins.elem (license.shortName or null) cudaLicenseNames)
                (nixpkgs.lib.toList pkg.meta.license);
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
            # crates/spiral-robotics-studio/Cargo.toml; wasm-bindgen refuses a mismatch.
            pkgs.wasm-bindgen-cli_0_2_126
            pkgs.binaryen # wasm-opt, used for release builds
          ];
          linuxNativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            llvmPackages.libclang
          ];
          linuxBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.v4l-utils
          ];
          # The Studio owns orchestration in Rust. The isolated worker uses the
          # Spiral Python relation API for random-access sample preparation and
          # native media-to-tensor conversion, then PyTorch for vision ACT.
          # Checkpoint publication remains in Rust.
          #
          # SpiralDB's native Python relation wheel is currently distributed
          # separately from nixpkgs. Development against a SpiralDB checkout
          # should override SPIRAL_ROBOTICS_TRAINING_PYTHON with that checkout's
          # Python environment until the native wheel has a stable package URL.
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
          sharedShell = {
            packages =
              [
                rustToolchain
                pkgs.actionlint
                pkgs.pkg-config
                llvmPackages.llvm
              ]
              ++ linuxNativeBuildInputs
              ++ linuxBuildInputs;

            PYTHONNOUSERSITE = "1";
            ASAN_SYMBOLIZER_PATH = "${llvmPackages.llvm}/bin/llvm-symbolizer";
            LIBCLANG_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux "${llvmPackages.libclang.lib}/lib";
          };
        in
        {
          packages.training-python = trainingPython;

          devShells.default = pkgs.mkShell (sharedShell // {
            packages =
              sharedShell.packages
              ++ [
                pkgs.cargo-fuzz
                pkgs.cargo-binutils
                pkgs.ffmpeg
                trainingPython
              ]
              ++ webBuildInputs;
            # Make `dx` use pinned tools rather than downloading its own copies
            # at build time.
            NO_DOWNLOADS = "1";
            # Keep the Python boundary reproducible and invisible to Rust
            # callers. An explicit --training-python still wins when testing a
            # CUDA-specific environment outside nixpkgs.
            SPIRAL_ROBOTICS_TRAINING_PYTHON = "${trainingPython}/bin/python";
          });

          # CI validates the Rust workspace and Python worker syntax without
          # downloading the multi-gigabyte tensor/CUDA training environment.
          devShells.ci = pkgs.mkShell (sharedShell // {
            packages = sharedShell.packages ++ [ pkgs.python312 ];
          });
        }
      );
}
