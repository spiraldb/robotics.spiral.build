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
          # Pin the official standalone release and its archive checksums here.
          # Rust builds never compile the embedded engine.
          adbcVersion = "0.16.41";
          adbcArchiveHashes = {
            aarch64-apple-darwin = "a794846ef442afae8ef23c648161b009ce96e2d021001b6ca66b7568bb4868db";
            aarch64-unknown-linux-gnu = "26176532a8c83ac10f1105646b6e4aa3f0cec4fd99ad71cd9e1d3212d0559e4a";
            x86_64-unknown-linux-gnu = "90752bfc12b167086834df3d0505a5a06ca6e801895710f879ad9aacf6de2324";
          };
          adbcTarget =
            {
              aarch64-darwin = "aarch64-apple-darwin";
              aarch64-linux = "aarch64-unknown-linux-gnu";
              x86_64-linux = "x86_64-unknown-linux-gnu";
            }
            .${system};
          adbcLibrary = "libadbc_driver_spiraldb${hostPlatform.extensions.sharedLibrary}";
          # The driver loads the video runtime from its own directory.
          videoRuntimeLibrary = "libspiral_video_runtime${hostPlatform.extensions.sharedLibrary}";
          # The archive leaves out the FFmpeg 9 libraries the video runtime
          # links. Headless is the smallest nixpkgs build that has them.
          ffmpegLibraries = lib.getLib pkgs.ffmpeg_9-headless;
          spiralAdbc = pkgs.stdenvNoCC.mkDerivation {
            pname = "spiral-adbc";
            version = adbcVersion;
            src = pkgs.fetchurl {
              url = "https://downloads.spiraldb.com/adbc/${adbcVersion}/adbc-driver-spiraldb-${adbcVersion}-${adbcTarget}.tar.gz";
              sha256 = adbcArchiveHashes.${adbcTarget};
            };
            nativeBuildInputs =
              lib.optionals hostPlatform.isLinux [ pkgs.autoPatchelfHook ]
              ++ lib.optionals hostPlatform.isDarwin [ pkgs.cctools ];
            buildInputs = lib.optionals hostPlatform.isLinux [
              pkgs.stdenv.cc.cc.lib
              ffmpegLibraries
            ];
            dontBuild = true;
            # On Linux, autoPatchelfHook points the runtime at FFmpeg. On macOS
            # the runtime names FFmpeg by @rpath and carries no rpath itself.
            installPhase = ''
              runHook preInstall
              mkdir -p "$out/lib"
              cp ${adbcLibrary} ${videoRuntimeLibrary} "$out/lib/"
            ''
            + lib.optionalString hostPlatform.isDarwin ''
              install_name_tool -add_rpath ${ffmpegLibraries}/lib "$out/lib/${videoRuntimeLibrary}"
            ''
            + ''
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
            ];
          };
          cargoDeps = rustPlatform.importCargoLock {
            lockFile = ./Cargo.lock;
            # The patched Dioxus checkout (Cargo.toml's [patch.crates-io]),
            # one hash for every crate it provides.
            outputHashes."const-serialize-0.8.0-alpha.0" = "sha256-1YJZcijcAXEQ//NxevLvOUfDu18jdzwOGnBztouwisE=";
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
              # The driver verifies TLS against the system's certificates.
              pkgs.dockerTools.caCertificates
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
                "SPIRAL_ROBOTICS_STUDIO_DB=${studioHome}/studio.sqlite3"
                # Spiral's sign-in returns browsers to this address's /callback.
                "SPIRAL_ROBOTICS_PUBLIC_URL=https://robotics.spiraldb.com"
                "SPIRAL_ROBOTICS_CLIENT_ID=client_PiwYy0njXU92"
                # The Modal services Score progress and training run on, in the
                # spiral-dev workspace; their shared token is a deployment
                # secret.
                "WARP_PROGRESS_URL=https://spiral-dev--warp-progress-warpprogress-web.modal.run"
                "TRAINING_SERVICE_URL=https://spiral-dev--spiral-robotics-training-control.modal.run"
                "SPIRALDB_ADBC_DRIVER=${spiralAdbc}/lib/${adbcLibrary}"
                "DIOXUS_PUBLIC_PATH=${studioWeb}"
              ];
              Volumes.${studioHome} = { };
              ExposedPorts."8080/tcp" = { };
              # Studio drains and stops its running work on Ctrl+C.
              StopSignal = "SIGINT";
              Labels = {
                "org.opencontainers.image.revision" = self.rev or self.dirtyRev;
                "com.spiraldb.adbc.version" = adbcVersion;
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
            SPIRALDB_ADBC_DRIVER = "${spiralAdbc}/lib/${adbcLibrary}";
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
            // lib.optionalAttrs hostPlatform.isLinux {
              LD_LIBRARY_PATH = lib.makeLibraryPath [ pkgs.vulkan-loader ];
              VK_ICD_FILENAMES = "${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${hostPlatform.parsed.cpu.name}.json";
            };
          developmentShell = sharedShell // {
            SPIRAL_ROBOTICS_CLIENT_ID = "client_aRm8njLb18Px";
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
            spiral-adbc = spiralAdbc;
            studio-server = studioServer;
            studio-web = studioWeb;
          }
          // lib.optionalAttrs hostPlatform.isLinux {
            hosted-image = hostedImage;
          };

          devShells.default = pkgs.mkShell developmentShell;
          devShells.ci = pkgs.mkShell ciShell;

          # Explicit validation environment until the public release includes
          # the upstream persistence fixes. No hand-written exports are needed.
          devShells.local-driver = pkgs.mkShell (
            developmentShell
            // {
              SPIRALDB_ADBC_DRIVER = "";
              shellHook = developmentShell.shellHook + ''
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
