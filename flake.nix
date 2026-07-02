{
  description = "Claude Commander - Terminal UI for managing Claude coding sessions";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
    # Rust toolchain with Android cross-compile targets, used ONLY by the
    # `client` dev shell (see devShells.client). The default shell never pulls it.
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, flake-utils, fenix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        craneLib = crane.mkLib pkgs;

        # .md files must survive the source filter:
        # crates/claude-commander-core/src/commander_prime.md is embedded into
        # the binary via include_str!. The src root is the whole tree, so every
        # workspace member under crates/ is in the build sandbox;
        # filterCargoSources keeps each crate's Rust/Cargo sources.
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            (pkgs.lib.hasSuffix ".md" path) || (craneLib.filterCargoSources path type);
          name = "source";
        };

        commonArgs = {
          inherit src;
          # The root is now a virtual workspace (no [package]), and the binary
          # crate inherits its version from [workspace.package] via
          # `version.workspace = true` — which crane's crateNameFromCargoToml
          # does not resolve. So pin pname to the binary crate and read the
          # concrete version straight from the root workspace manifest.
          pname = "claude-commander";
          version =
            (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          # Build only the TUI/CLI binary crate; the server crate
          # (claude-commander-server, publish = false, axum/tower/hyper) is
          # excluded from the Nix package the same way it is from
          # default-members.
          cargoExtraArgs = "-p claude-commander";
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            pkg-config
            makeWrapper
          ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.apple-sdk_15
          ];

          # rodio (conversation-mode audio) links ALSA via cpal on Linux.
          buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            pkgs.alsa-lib
          ];
        };

        # Build only dependencies (cached separately for incremental rebuilds)
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        claude-commander = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          # Some tests require a real git repo, which isn't available in the Nix sandbox
          doCheck = false;

          # tmux and git are required at runtime
          postFixup = ''
            wrapProgram $out/bin/claude-commander \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.tmux pkgs.git ]}
          '';

          meta = with pkgs.lib; {
            description = "A high-performance terminal UI for managing Claude coding sessions";
            homepage = "https://github.com/sizeak/claude-commander";
            license = licenses.mit;
            mainProgram = "claude-commander";
          };
        });
        # ---- Client (Flutter + Rust) dev-shell toolchain ----
        # Heavy Flutter + Android NDK toolchain for the in-repo `client/` app,
        # kept entirely out of the default shell so core TUI/CLI/server
        # contributors never pull it. nixpkgs is re-imported with unfree + Android
        # SDK licence acceptance, scoped to this shell only. All of these bindings
        # are lazy: `nix develop` (default) never evaluates or builds them.
        clientPkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
            android_sdk.accept_license = true;
          };
        };
        # Slim host-only toolchain for the CI shell (no Android cross targets).
        fenixStable = fenix.packages.${system}.stable.toolchain;
        clientRust = fenix.packages.${system}.combine [
          fenixStable
          fenix.packages.${system}.targets.aarch64-linux-android.stable.rust-std
          fenix.packages.${system}.targets.armv7-linux-androideabi.stable.rust-std
          fenix.packages.${system}.targets.x86_64-linux-android.stable.rust-std
          fenix.packages.${system}.targets.i686-linux-android.stable.rust-std
        ];
        clientAndroid = clientPkgs.androidenv.composeAndroidPackages {
          # Platform 36 is required by recent plugins (path_provider_android →
          # jni_flutter compileSdk 36); the Nix SDK is read-only so every needed
          # platform must be listed here rather than auto-installed by Gradle.
          platformVersions = [ "34" "35" "36" ];
          buildToolsVersions = [ "34.0.0" "35.0.0" "36.0.0" ];
          ndkVersions = [ "28.0.13004108" ];
          cmakeVersions = [ "3.22.1" ];
          includeNDK = true;
          includeEmulator = true;
          includeSystemImages = true;
          # x86_64 ABI gives full KVM hardware acceleration on Linux/x86_64
          # hosts. On darwin the emulator isn't KVM-bootable, but these values
          # still evaluate — androidenv downloads per-host artefacts lazily.
          systemImageTypes = [ "google_apis" ];
          abiVersions = [ "x86_64" ];
          cmdLineToolsVersion = "13.0";
        };
        clientAndroidSdkRoot = "${clientAndroid.androidsdk}/libexec/android-sdk";
        clientNdkVersion = "28.0.13004108";
        # cargokit (flutter_rust_bridge's native-build glue, vendored under
        # client/rust_builder/cargokit) hard-requires `rustup` — it queries
        # `rustup toolchain list` / `rustup target list --installed` and builds
        # via `rustup run <toolchain> cargo build`, with no plain-cargo
        # fallback. These shells pin Rust via fenix instead of rustup, so
        # provide a shim (bound to a specific Nix toolchain) that answers
        # cargokit's queries and execs `rustup run`'s command directly (the
        # toolchain name is ignored — Nix already pinned it). Toolchain/target
        # *installation* is Nix's job: the shim fails loudly so a missing target
        # is fixed in flake.nix, not auto-downloaded.
        #
        # NOTE: cargokit resolves `rustup` from `$HOME/.cargo/bin` *before* PATH
        # (rustup.dart `executablePath`), so on hosts with a real
        # `~/.cargo/bin/rustup` (e.g. GitHub runners) this shim is bypassed. CI
        # shadows that path with the shim in the e2e step — see ci.yml.
        mkRustupShim = toolchain: clientPkgs.writeShellScriptBin "rustup" ''
          set -eu
          # Prepend the pinned toolchain so the shim and the cargo/rustc it execs
          # are found regardless of the caller's PATH — cargokit may invoke us
          # from a build subprocess with a reduced environment.
          export PATH="${toolchain}/bin''${PATH:+:$PATH}"
          cmd="''${1:-}"; [ $# -gt 0 ] && shift
          case "$cmd" in
            toolchain)
              sub="''${1:-}"
              if [ "$sub" = "list" ]; then
                echo "stable-$(rustc -vV | sed -n 's/^host: //p') (default)"
              else
                echo "rustup shim: 'rustup toolchain $sub' is unsupported — toolchains come from Nix (fenix); edit flake.nix" >&2
                exit 1
              fi ;;
            target)
              sub="''${1:-}"
              if [ "$sub" = "list" ]; then
                sysroot="$(rustc --print sysroot)"
                # `if`, not `[ … ] &&`: under `set -e` the AND-list form makes a
                # trailing non-target entry (e.g. `etc/`) the loop's — and the
                # script's — exit status, which aborts cargokit. Which entry
                # sorts last is platform-dependent (aarch64-apple-darwin sorts
                # BEFORE etc), so the && form breaks exactly on macOS.
                for d in "$sysroot"/lib/rustlib/*/; do
                  if [ -d "$d/lib" ]; then basename "$d"; fi
                done
              else
                echo "rustup shim: 'rustup target $sub' is unsupported — add the target to the fenix toolchain in flake.nix" >&2
                exit 1
              fi ;;
            run)
              shift # toolchain name — pinned by Nix, ignored
              exec "$@" ;;
            *)
              echo "rustup shim: unsupported command '$cmd'" >&2
              exit 1 ;;
          esac
        '';
        # `client` carries the Android-cross toolchain; `clientCi` the slim
        # host-only one — so each shell's shim exposes exactly its own targets.
        clientRustupShim = mkRustupShim clientRust;
        clientCiRustupShim = mkRustupShim fenixStable;
      in
      {
        checks = {
          inherit claude-commander;
        };

        packages = {
          claude-commander = claude-commander;
          default = claude-commander;
        };

        apps.default = {
          type = "app";
          program = "${claude-commander}/bin/claude-commander";
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgs; [
            rust-analyzer
            tmux
            git
            pkg-config
          ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            alsa-lib
          ];
        };

        # Flutter + Rust + Android NDK toolchain for the in-repo `client/` app
        # (Android-first, iOS + desktop to follow). Enter with
        # `nix develop .#client`. Separate from the default shell on purpose —
        # only client contributors pull this.
        devShells.client = clientPkgs.mkShell {
          name = "claude-commander-client";
          # Cross-platform toolchain: Rust + Android targets, the Android SDK/NDK,
          # JDK, and the Flutter/Dart/codegen/native-build tools. Usable on both
          # Linux and macOS — the Linux-desktop GTK/X11 stack is appended only on
          # Linux (macOS desktop is Cocoa, built via Xcode, not these libs).
          buildInputs = [
            clientRust
            clientRustupShim
            clientAndroid.androidsdk
            clientPkgs.jdk17
          ] ++ (with clientPkgs; [
            flutter
            dart
            cargo-ndk
            # flutter_rust_bridge codegen — fall back to `cargo install` if this
            # attr is ever absent from the nixpkgs pin (see client/README.md).
            flutter_rust_bridge_codegen
            cmake
            ninja
            pkg-config
            clang
            llvmPackages.libclang
          ]) ++ clientPkgs.lib.optionals clientPkgs.stdenv.hostPlatform.isLinux (with clientPkgs; [
            # Linux desktop (bonus target) GTK / build deps Flutter needs.
            gtk3
            glib
            pcre2
            libepoxy
            libx11
            # flutter_secure_storage_linux links libsecret (needs a running
            # secret service at runtime, e.g. gnome-keyring).
            libsecret
          ]);

          # Used by flutter_rust_bridge / bindgen to find libclang.
          LIBCLANG_PATH = "${clientPkgs.llvmPackages.libclang.lib}/lib";

          shellHook = ''
            export ANDROID_HOME="${clientAndroidSdkRoot}"
            export ANDROID_SDK_ROOT="${clientAndroidSdkRoot}"
            export ANDROID_NDK_ROOT="${clientAndroidSdkRoot}/ndk/${clientNdkVersion}"
            export ANDROID_NDK_HOME="$ANDROID_NDK_ROOT"
            # Gradle (android/app/build.gradle.kts) reads this so AGP uses the
            # Nix-provided NDK rather than installing Flutter's default.
            export ANDROID_NDK_VERSION="${clientNdkVersion}"
            export JAVA_HOME="${clientPkgs.jdk17}"
            # Point Flutter at the Nix-provided SDK and silence analytics noise.
            flutter config --no-analytics >/dev/null 2>&1 || true
            flutter config --android-sdk "$ANDROID_SDK_ROOT" >/dev/null 2>&1 || true

            # ---- Linux desktop: EGL + Rust cdylib discovery ----
            # Flutter Linux uses system Mesa EGL (libEGL_mesa.so) for display
            # rendering.  The Nix-built libepoxy probes for EGL at runtime; it must
            # find /usr/lib/libEGL_mesa.so (Arch system Mesa), so prepend /usr/lib.
            # Android/iOS toolchains are unaffected by this path entry.
            export LD_LIBRARY_PATH="/usr/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            # flutter_rust_bridge's generated ioDirectory is 'rust/target/release/',
            # but a debug build puts the cdylib in rust/target/debug/.  Symlink
            # release -> debug so Dart's dlopen via FRB finds the library after the
            # first `flutter build linux --debug` or `flutter_rust_bridge_codegen
            # generate`.  Flutter release builds are unaffected: cargokit uses a
            # separate target dir (build/…/plugins/…/cargokit_build).
            # WARNING: with this symlink, a manual `cargo build --release` in
            # client/rust writes release artefacts into target/debug, mixing
            # profiles. Use cargokit/Flutter for release builds, or remove the
            # symlink before building release by hand.
            if [ -d "client/rust" ]; then
              mkdir -p client/rust/target
              ln -sfT debug client/rust/target/release 2>/dev/null || true
            fi

            # Create the Android emulator AVD on first use (idempotent: skipped
            # if it already exists). x86_64 google_apis image for Android 35 →
            # full KVM acceleration on Linux. Boot it (not done here — would
            # block shell entry) with:
            #   emulator -avd cctest -no-window -gpu swiftshader_indirect \
            #            -no-audio -no-boot-anim -accel on &
            #   adb wait-for-device
            #   until adb shell getprop sys.boot_completed 2>/dev/null | grep -q 1; do sleep 3; done
            if ! avdmanager list avd 2>/dev/null | grep -q 'Name: cctest'; then
              echo "Creating Android emulator AVD 'cctest' (android-35 google_apis x86_64)..."
              avdmanager create avd -n cctest \
                -k "system-images;android-35;google_apis;x86_64" \
                --device pixel_6 --force >/dev/null 2>&1 \
                && echo "AVD 'cctest' created." \
                || echo "AVD creation skipped (non-Linux host or no /dev/kvm — emulator needs Linux/KVM to boot)."
            fi
            echo "entered claude-commander client dev shell (flutter + rust + android ndk)"
          '';
        };

        # Slim CI shell for the client's automated tests: Flutter + host Rust +
        # the Linux-desktop stack + tmux/git/xvfb, but WITHOUT the Android
        # SDK/NDK/emulator. The client e2e runs on the Linux **desktop** target
        # (`flutter test integration_test -d linux`), so Android isn't needed and
        # would only bloat the CI image. Local contributors keep using `.#client`.
        devShells.clientCi = clientPkgs.mkShell {
          name = "claude-commander-client-ci";
          buildInputs = [
            # Host-only Rust (no Android targets): cargokit cross-builds the
            # cdylib for the linux desktop target during the e2e bundle build.
            fenixStable
            clientCiRustupShim
          ] ++ (with clientPkgs; [
            flutter
            dart
            cmake
            ninja
            pkg-config
            clang
            llvmPackages.libclang
            # client/tool/e2e.sh runtime: the server needs tmux + git; the health
            # poll uses curl; xvfb-run gives the linux bundle a headless display.
            tmux
            git
            curl
            xvfb-run
            # Linux desktop GTK stack + software GL (Mesa llvmpipe) for headless
            # rendering under xvfb.
            gtk3
            glib
            pcre2
            libepoxy
            libx11
            libsecret
            mesa
            libGL
          ]);

          LIBCLANG_PATH = "${clientPkgs.llvmPackages.libclang.lib}/lib";
          # Force software GL so the Flutter linux bundle renders under xvfb with
          # no GPU present (Mesa llvmpipe).
          LIBGL_ALWAYS_SOFTWARE = "1";

          shellHook = ''
            export LD_LIBRARY_PATH="${clientPkgs.lib.makeLibraryPath [ clientPkgs.libGL clientPkgs.mesa clientPkgs.libepoxy ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            flutter config --no-analytics >/dev/null 2>&1 || true
            flutter config --enable-linux-desktop >/dev/null 2>&1 || true
            # frb's generated ioDirectory is rust/target/release/, but a debug
            # cdylib lands in rust/target/debug/ — symlink so dlopen finds it.
            if [ -d "client/rust" ]; then
              mkdir -p client/rust/target
              ln -sfT debug client/rust/target/release 2>/dev/null || true
            fi
          '';
        };
      }
    );
}
