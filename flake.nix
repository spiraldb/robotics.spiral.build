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
      self,
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
          inherit (pkgs) lib;
          inherit (pkgs.stdenv) hostPlatform;
          llvmPackages = pkgs.llvmPackages_latest;
          # This flake owns the toolchain, tools, and exported build environment.
          # One Rust pin for the shells and the packages.
          rustPin = {
            channel = "nightly-2026-10-01";
            # The studio's web interface compiles to WebAssembly.
            targets = [ "wasm32-unknown-unknown" ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchain (
            rustPin
            // {
              components = [
                "rust-src"
                "rust-analyzer"
                "rustfmt"
                "clippy"
              ];
            }
          );
          # The packages build with the same compiler but none of the shells'
          # components: with rust-src installed, panic locations name the
          # toolchain's store path, which would put all of it in the image.
          packageRustToolchain = pkgs.rust-bin.fromRustupToolchain (rustPin // { profile = "minimal"; });
          rustPlatform = pkgs.makeRustPlatform {
            cargo = packageRustToolchain;
            rustc = packageRustToolchain;
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
          linuxBuildInputs = lib.optionals hostPlatform.isLinux [
            pkgs.v4l-utils
          ];
          # Spiral's data-plane runtime, the library spiral-sdk loads: built
          # from the SpiralDB commit Cargo.toml pins the SDK at, with that
          # repository's own toolchain and Cargo configuration. Its private
          # source is fetched with the caller's git credentials.
          spiralRev = (lib.importTOML ./Cargo.toml).workspace.dependencies.spiral-sdk.rev;
          spiralSource = builtins.fetchGit {
            url = "https://github.com/spiraldb/spiraldb.git";
            rev = spiralRev;
            allRefs = true;
          };
          spiralToolchain = pkgs.rust-bin.fromRustupToolchainFile "${spiralSource}/rust-toolchain.toml";
          spiralRustPlatform = pkgs.makeRustPlatform {
            cargo = spiralToolchain;
            rustc = spiralToolchain;
          };
          runtimeLibrary = "libspiral_data_plane_runtime${hostPlatform.extensions.sharedLibrary}";
          # The `vortex` crate's docs include its workspace README, which
          # vendoring a Git crate leaves behind; a build step puts it back,
          # from the Vortex commit `lockFile` pins.
          restoreVortexReadme =
            lockFile:
            let
              vortexRev = lib.head (
                lib.concatMap (
                  package: lib.optional (package.name == "vortex") (lib.last (lib.splitString "#" package.source))
                ) (lib.importTOML lockFile).package
              );
              vortexSource = builtins.fetchGit {
                url = "https://github.com/vortex-data/vortex.git";
                rev = vortexRev;
                allRefs = true;
              };
            in
            ''
              cp ${vortexSource}/README.md "$NIX_BUILD_TOP/cargo-vendor-dir/vortex-0.1.0/README.md"
            '';
          spiralRuntime = spiralRustPlatform.buildRustPackage {
            pname = "spiral-data-plane-runtime";
            version = builtins.substring 0 9 spiralRev;
            src = spiralSource;
            cargoLock = {
              lockFile = "${spiralSource}/Cargo.lock";
              allowBuiltinFetchGit = true;
            };
            cargoBuildFlags = [
              "--package"
              "spiral-data-plane-runtime"
              "--lib"
              # OpenSSL is compiled into the library, so it needs none at run time.
              "--features"
              "bundled-openssl"
            ];
            nativeBuildInputs = [
              pkgs.protobuf
              pkgs.pkg-config
              pkgs.perl
              pkgs.cmake
            ]
            ++ nativeBuildInputs;
            LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
            preBuild = restoreVortexReadme "${spiralSource}/Cargo.lock";
            doCheck = false;
            installPhase = ''
              runHook preInstall
              mkdir -p "$out/lib"
              cp "$(find target -path '*/release/${runtimeLibrary}' | head -n 1)" "$out/lib/"
              runHook postInstall
            '';
          };
          workspaceVersion = (lib.importTOML ./Cargo.toml).workspace.package.version;
          workspaceSource = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./.cargo
              ./crates
              ./robot-connector
            ];
          };
          # Vendoring finds each Git dependency's crates with `cargo metadata`
          # over that repository's workspace, and SpiralDB's declares artifact
          # dependencies, which Cargo parses only with `-Z bindeps`.
          vendoringPlatform = pkgs.makeRustPlatform {
            cargo = pkgs.writeShellScriptBin "cargo" ''
              CARGO_UNSTABLE_BINDEPS=true exec ${packageRustToolchain}/bin/cargo "$@"
            '';
            rustc = packageRustToolchain;
          };
          cargoDeps = vendoringPlatform.importCargoLock {
            lockFile = ./Cargo.lock;
            # Spiral's SDK and Vortex come from Git; SpiralDB's private
            # repository through the caller's git credentials.
            allowBuiltinFetchGit = true;
            # The patched Dioxus checkout (Cargo.toml's [patch.crates-io]),
            # one hash for every crate it provides.
            outputHashes."const-serialize-0.8.0-alpha.0" =
              "sha256-1YJZcijcAXEQ//NxevLvOUfDu18jdzwOGnBztouwisE=";
          };
          studioServer = rustPlatform.buildRustPackage {
            pname = "spiral-robotics-backend";
            version = workspaceVersion;
            src = workspaceSource;
            inherit cargoDeps;
            cargoBuildFlags = [
              "--package"
              "spiral-robotics-backend"
            ];
            # OpenSSL's source build, which the server carries, configures
            # itself with Perl.
            nativeBuildInputs = [ pkgs.perl ];
            preBuild = restoreVortexReadme ./Cargo.lock;
            doCheck = false;
            meta.mainProgram = "spiral-robotics-backend";
          };
          # The `public/` directory the server serves the web interface from.
          studioWeb = pkgs.stdenv.mkDerivation {
            pname = "spiral-robotics-web";
            version = workspaceVersion;
            src = workspaceSource;
            inherit cargoDeps;
            nativeBuildInputs = [
              rustPlatform.cargoSetupHook
              packageRustToolchain
              tailwind
              pkgs.wasm-bindgen-cli_0_2_126
            ]
            ++ webBuildInputs;
            buildPhase = ''
              runHook preBuild
              dx bundle --web --release --debug-symbols false --frozen \
                --package spiral-robotics-frontend --out-dir bundle
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              cp -r bundle/public "$out"
              runHook postInstall
            '';
          };
          # Studio as a Linux server for the hosted VM. The `studio` user owns
          # HOME, the one volume: Studio's SQLite state and the Spiral sign-in
          # (under ~/.config).
          studioHome = "/var/lib/spiral-robotics";
          hostedImage = pkgs.dockerTools.streamLayeredImage {
            name = "spiral-robotics-studio";
            contents = [
              # The Spiral SDK verifies TLS against the system's certificates.
              pkgs.dockerTools.caCertificates
              # Spiral's engine resolves named time zones, `UTC` included,
              # through a time zone database (a Location's `last_modified`).
              pkgs.tzdata
              (pkgs.dockerTools.fakeNss.override {
                extraPasswdLines = [ "studio:x:10001:10001::${studioHome}:/sbin/nologin" ];
                extraGroupLines = [ "studio:x:10001:" ];
              })
            ];
            # Studio writes temporary files to /tmp.
            fakeRootCommands = ''
              mkdir -p ./tmp .${studioHome}
              chmod 1777 ./tmp
              chown 10001:10001 .${studioHome}
            '';
            config = {
              User = "10001:10001";
              Entrypoint = [
                (lib.getExe studioServer)
                "--bind"
                "0.0.0.0:8080"
              ];
              Env = [
                "HOME=${studioHome}"
                "TZDIR=${pkgs.tzdata}/share/zoneinfo"
                "SPIRAL_ROBOTICS_STUDIO_DB=${studioHome}/studio.sqlite3"
                # Spiral's sign-in returns browsers to this address's /callback.
                "SPIRAL_ROBOTICS_PUBLIC_URL=https://robotics.spiraldb.com"
                "SPIRAL_ROBOTICS_CLIENT_ID=client_3CMizYLaPa3h"
                # The Modal services Score progress, Track objects, and
                # training run on, in the spiral-dev workspace; their shared
                # token is a deployment secret.
                "WARP_PROGRESS_URL=https://spiral-dev--warp-progress-warpprogress-web.modal.run"
                "OBJECT_TRACKING_URL=https://spiral-dev--spiral-robotics-object-tracking-objecttracking-web.modal.run"
                "TRAINING_SERVICE_URL=https://spiral-dev--spiral-robotics-training-control.modal.run"
                "SPIRAL_DATA_PLANE_LIBRARY=${spiralRuntime}/lib/${runtimeLibrary}"
                # LeRobot imports encode every camera again as H.264.
                "SPIRAL_ROBOTICS_FFMPEG=${lib.getExe' pkgs.ffmpeg_9-headless "ffmpeg"}"
                "DIOXUS_PUBLIC_PATH=${studioWeb}"
              ];
              Volumes.${studioHome} = { };
              ExposedPorts."8080/tcp" = { };
              # Studio drains and stops its running work on Ctrl+C.
              StopSignal = "SIGINT";
              Labels = {
                "org.opencontainers.image.revision" = self.rev or self.dirtyRev;
                "com.spiraldb.sdk.rev" = spiralRev;
              };
            };
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
            LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
            GECKODRIVER = lib.getExe pkgs.geckodriver;
            # Select the pinned browser instead of a host Firefox installation.
            WASM_BINDGEN_TEST_WEBDRIVER_JSON = pkgs.writeText "studio-webdriver.json" (
              builtins.toJSON {
                "moz:firefoxOptions" = {
                  binary =
                    if hostPlatform.isDarwin then
                      "${pkgs.firefox}/Applications/Firefox.app/Contents/MacOS/firefox"
                    else
                      lib.getExe pkgs.firefox;
                  args = [
                    "--width=1280"
                    "--height=800"
                  ];
                };
              }
            );
          };
          # Mesa's software Vulkan gives the viewport's off-screen render test a
          # graphics adapter on a Linux runner without a GPU.
          ciShell =
            sharedShell
            // {
              # The storage tests encode camera video with FFmpeg, as LeRobot
              # imports do.
              packages = sharedShell.packages ++ [ pkgs.ffmpeg_9-headless ];
            }
            // lib.optionalAttrs hostPlatform.isLinux {
              LD_LIBRARY_PATH = lib.makeLibraryPath [ pkgs.vulkan-loader ];
              VK_ICD_FILENAMES = "${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${hostPlatform.parsed.cpu.name}.json";
            };
          developmentShell = sharedShell // {
            SPIRAL_ROBOTICS_CLIENT_ID = "client_sQWdRwk7ZTBd";
            # Studio reaches Spiral through this runtime; tests never load it.
            SPIRAL_DATA_PLANE_LIBRARY = "${spiralRuntime}/lib/${runtimeLibrary}";
            packages = sharedShell.packages ++ [ pkgs.ffmpeg ] ++ webBuildInputs;
            # Studio's provider keys for local runs come from the checkout's
            # untracked .env.local, in the variables the hosted image is given.
            shellHook = ''
              if studio_env=$(git rev-parse --show-toplevel 2>/dev/null)/.env.local \
                && [ -f "$studio_env" ]; then
                set -a
                . "$studio_env"
                set +a
              fi
              unset studio_env
            '';
          };
        in
        {
          formatter = pkgs.nixfmt;
          packages = {
            spiral-runtime = spiralRuntime;
            studio-server = studioServer;
            studio-web = studioWeb;
          }
          // lib.optionalAttrs hostPlatform.isLinux {
            hosted-image = hostedImage;
          };

          devShells.default = pkgs.mkShell developmentShell;
          devShells.ci = pkgs.mkShell ciShell;

        }
      );
}
