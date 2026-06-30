{
  description = "Claude Commander - Terminal UI for managing Claude coding sessions";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
    # Rust toolchain with Android cross-compile targets, used ONLY by the
    # `mobile` dev shell (see devShells.mobile). The default shell never pulls it.
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
        clientRust = fenix.packages.${system}.combine [
          fenix.packages.${system}.stable.toolchain
          fenix.packages.${system}.targets.aarch64-linux-android.stable.rust-std
          fenix.packages.${system}.targets.armv7-linux-androideabi.stable.rust-std
          fenix.packages.${system}.targets.x86_64-linux-android.stable.rust-std
          fenix.packages.${system}.targets.i686-linux-android.stable.rust-std
        ];
        clientAndroid = clientPkgs.androidenv.composeAndroidPackages {
          platformVersions = [ "34" "35" ];
          buildToolsVersions = [ "34.0.0" "35.0.0" ];
          ndkVersions = [ "28.0.13004108" ];
          cmakeVersions = [ "3.22.1" ];
          includeNDK = true;
          includeEmulator = false;
          includeSystemImages = false;
          cmdLineToolsVersion = "13.0";
        };
        clientAndroidSdkRoot = "${clientAndroid.androidsdk}/libexec/android-sdk";
        clientNdkVersion = "28.0.13004108";
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
          buildInputs = [
            clientRust
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
            # Linux desktop (bonus target) GTK / build deps Flutter needs.
            gtk3
            glib
            pcre2
            libepoxy
            libx11
          ]);

          # Used by flutter_rust_bridge / bindgen to find libclang.
          LIBCLANG_PATH = "${clientPkgs.llvmPackages.libclang.lib}/lib";

          shellHook = ''
            export ANDROID_HOME="${clientAndroidSdkRoot}"
            export ANDROID_SDK_ROOT="${clientAndroidSdkRoot}"
            export ANDROID_NDK_ROOT="${clientAndroidSdkRoot}/ndk/${clientNdkVersion}"
            export ANDROID_NDK_HOME="$ANDROID_NDK_ROOT"
            export JAVA_HOME="${clientPkgs.jdk17}"
            # Point Flutter at the Nix-provided SDK and silence analytics noise.
            flutter config --no-analytics >/dev/null 2>&1 || true
            flutter config --android-sdk "$ANDROID_SDK_ROOT" >/dev/null 2>&1 || true
            echo "entered claude-commander client dev shell (flutter + rust + android ndk)"
          '';
        };
      }
    );
}
