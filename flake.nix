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
          # This flake owns the toolchain, tools, and exported build environment.
          rustToolchain = pkgs.rust-bin.nightly."2026-06-29".default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "rustfmt"
              "clippy"
            ];
            # The studio's web interface compiles to WebAssembly.
            targets = [ "wasm32-unknown-unknown" ];
          };
          # nixpkgs kept Tailwind 3 under the unsuffixed name for a while.
          tailwind = pkgs.tailwindcss_4 or pkgs.tailwindcss;
          # `dx` builds the web client and native server as one fullstack app,
          # compiles the declared Tailwind input, and bundles both. With
          # NO_DOWNLOADS set it takes every tool, Tailwind included, from PATH
          # instead of fetching its own, so the whole toolchain is pinned here.
          webBuildInputs = [
            pkgs.dioxus-cli # fullstack dx bundle / dx serve
            pkgs.binaryen # wasm-opt, used for release builds
          ];
          browserTestInputs = [
            # Includes wasm-bindgen-test-runner; must match the frontend pin.
            pkgs.wasm-bindgen-cli_0_2_126
            pkgs.firefox
            pkgs.geckodriver
          ];
          nativeBuildInputs = [
            llvmPackages.libclang
          ];
          linuxBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.v4l-utils
          ];
          # Pin the official standalone release and its archive checksums here.
          # Rust builds never compile the embedded engine.
          adbcVersion = "0.16.26";
          adbcArchiveHashes = {
            aarch64-apple-darwin = "37df6fd5c8601c2d97675742a9646d4023e8c3239dac0d2cf3a0266de3bdd427";
            aarch64-unknown-linux-gnu = "1221685bc4dbe1cadd4ccc2389d8b83aa81786749b0f86ddf2a6d65f16365ada";
            x86_64-unknown-linux-gnu = "74f2c17200ca653cac22d5423603581222464c496787ea450768c5d649794982";
          };
          adbcTarget =
            {
              aarch64-darwin = "aarch64-apple-darwin";
              aarch64-linux = "aarch64-unknown-linux-gnu";
              x86_64-linux = "x86_64-unknown-linux-gnu";
            }
            .${system};
          adbcLibrary = "libadbc_driver_spiraldb${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}";
          spiralAdbc = pkgs.stdenvNoCC.mkDerivation {
            pname = "spiral-adbc";
            version = adbcVersion;
            src = pkgs.fetchurl {
              url = "https://downloads.spiraldb.com/adbc/${adbcVersion}/adbc-driver-spiraldb-${adbcVersion}-${adbcTarget}.tar.gz";
              sha256 = adbcArchiveHashes.${adbcTarget};
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
            packages = [
              rustToolchain
              pkgs.git
              pkgs.actionlint
              pkgs.pkg-config
              pkgs.python312
              pkgs.ruff
              tailwind
              pkgs.uv
            ]
            ++ nativeBuildInputs
            ++ linuxBuildInputs
            ++ browserTestInputs;
            NO_DOWNLOADS = "1";
            SPIRALDB_ADBC_DRIVER = "${spiralAdbc}/lib/${adbcLibrary}";
            LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
            GECKODRIVER = pkgs.lib.getExe pkgs.geckodriver;
            # Select the pinned browser instead of a host Firefox installation.
            WASM_BINDGEN_TEST_WEBDRIVER_JSON = pkgs.writeText "studio-webdriver.json" (
              builtins.toJSON {
                "moz:firefoxOptions" = {
                  binary =
                    if pkgs.stdenv.isDarwin then
                      "${pkgs.firefox}/Applications/Firefox.app/Contents/MacOS/firefox"
                    else
                      pkgs.lib.getExe pkgs.firefox;
                  args = [
                    "--width=1280"
                    "--height=800"
                  ];
                };
              }
            );
          };
          developmentShell = sharedShell // {
            packages =
              sharedShell.packages
              ++ [ pkgs.ffmpeg ]
              ++ webBuildInputs;
          };
        in
        {
          formatter = pkgs.nixfmt;
          packages.spiral-adbc = spiralAdbc;

          devShells.default = pkgs.mkShell developmentShell;
          devShells.ci = pkgs.mkShell sharedShell;

          # Explicit validation environment until the public release includes
          # the upstream persistence fixes. No hand-written exports are needed.
          devShells.local-driver = pkgs.mkShell (
            developmentShell
            // {
              SPIRALDB_ADBC_DRIVER = "";
              shellHook = ''
                if ! studio_root=$(git rev-parse --show-toplevel); then
                  echo "Enter the local-driver shell from the Studio checkout." >&2
                  return 1
                fi
                export SPIRALDB_ADBC_DRIVER="$studio_root/target/release/${adbcLibrary}"
                unset studio_root
                if [ ! -f "$SPIRALDB_ADBC_DRIVER" ]; then
                  echo "Build the upstream ADBC library first; expected $SPIRALDB_ADBC_DRIVER" >&2
                  return 1
                fi
              '';
            }
          );
        }
      );
}
