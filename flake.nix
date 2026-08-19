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

        # `src` is the package derivation's only source input: Nix hashes the
        # filtered tree and that hash lands in the .drv, so every file admitted
        # here forces a full (fat-LTO) rebuild when it changes — whether or not
        # cargo ever opens it. Admit the workspace's Rust/Cargo sources and
        # nothing else. `scripts/check-nix-src-filter.sh` guards both directions
        # of this and runs in CI.
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            let
              rel = pkgs.lib.removePrefix "${toString ./.}/" (toString path);
              # Top-level subtrees are default-deny: an allow-list means a new
              # directory can't quietly start invalidating the build, and it
              # prunes each subtree whole rather than leaving an empty directory
              # behind (an empty dir is still part of the hash, so `docs/` alone
              # surviving would make `docs/…` the next thing to force a
              # rebuild). `crates/` is the workspace; `.cargo/` is admitted
              # because cargo reads `.cargo/config.toml` from the workspace root
              # — there is none today, and one added later must take effect
              # rather than being silently dropped.
              #
              # This is what excludes `client/`, whose 13 Rust/Cargo files
              # (incl. frb_generated.rs and its own Cargo.lock) were landing in
              # the hash: root Cargo.toml `exclude`s it and no workspace crate
              # path-depends on it, so a Flutter-client commit was rebuilding a
              # binary it is not part of. That costs no client coverage —
              # nothing in this flake builds the Flutter app (client/ appears
              # only as the `client`/`clientCi` dev shells) and CI's `client`
              # job tests it from the checked-out worktree, never from `src`.
              prunedTopDir =
                type == "directory"
                && !(pkgs.lib.hasInfix "/" rel)
                && !(builtins.elem rel [ "crates" ".cargo" ]);
              # crates/claude-commander-core/src/commander_prime.md is embedded
              # via include_str!, so .md must survive the filter — but scoped to
              # crates/, or README/docs/CLAUDE.md invalidate the build too.
              isCrateMarkdown =
                pkgs.lib.hasPrefix "crates/" rel && pkgs.lib.hasSuffix ".md" rel;
            in
            !prunedTopDir && (isCrateMarkdown || craneLib.filterCargoSources path type);
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
          ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            # cpal's `pipewire` backend builds pipewire-sys/libspa-sys, whose
            # build scripts run bindgen; bindgenHook supplies LIBCLANG_PATH.
            pkgs.rustPlatform.bindgenHook
          ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.apple-sdk_15
          ];

          # cpal (conversation-mode audio) links both libpipewire (its default
          # host) and ALSA (its runtime fallback) on Linux.
          buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            pkgs.alsa-lib
            pkgs.pipewire
          ];

          # cpal's `pipewire` backend pulls libspa-sys/pipewire-sys. Their bindgen
          # step calls `clang_macro_fallback` to evaluate cast macros like
          # `SPA_ID_INVALID` ((uint32_t)0xffffffff); that probe writes a precompiled
          # header into the crate's OWN source dir. Crane vendors deps read-only in
          # the Nix store, so the write fails and bindgen *silently drops* the macro
          # → libspa fails to compile with "cannot find value `SPA_ID_INVALID`".
          # (`nix develop`/plain cargo use a writable registry, so they're immune —
          # only the sealed `nix build` hits this.) `configureCargoVendoredDepsHook`
          # points cargo at the read-only `$cargoVendorDir` during preConfigure; here
          # in preBuild (which runs after it) we copy the vendored sources somewhere
          # writable and repoint cargo's source replacement at the copy.
          preBuild = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
            if [ -n "''${cargoVendorDir:-}" ] && [ -n "''${CARGO_HOME:-}" ] && [ -f "$CARGO_HOME/config.toml" ]; then
              writableVendor="$TMPDIR/cc-writable-cargo-vendor"
              rm -rf "$writableVendor"
              cp -rL --no-preserve=mode "$cargoVendorDir" "$writableVendor"
              chmod -R u+w "$writableVendor"
              sed -i "s|$cargoVendorDir|$writableVendor|g" "$CARGO_HOME/config.toml"
            fi
          '';
        };

        mkClaudeCommander = extraArgs:
          let
            args = commonArgs // extraArgs;
          in
          craneLib.buildPackage (args // {
            # Build only dependencies (cached separately for incremental
            # rebuilds). Derived from `args`, not `commonArgs`: a cargo profile
            # override re-fingerprints every dependency, so a variant sharing the
            # default artifacts would rebuild the whole graph anyway.
            cargoArtifacts = craneLib.buildDepsOnly args;

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

        claude-commander = mkClaudeCommander { };

        # CI-only variant — NOT for distribution, and deliberately absent from
        # `checks` so `nix flake check` still means the real package.
        #
        # `[profile.release]`'s `lto = true` + `codegen-units = 1` re-optimise the
        # whole dependency graph in one largely serial LLVM pass, which is ~11 of
        # the Nix CI job's 12 minutes. Pull requests build this instead: it still
        # exercises everything that actually breaks — the source filter above, the
        # read-only-vendor `preBuild` workaround, bindgen inside the sandbox, and
        # `wrapProgram` — and only skips fat LTO. Pushes to `main` build this *and*
        # the real `packages.default`, so the profile users get from AUR /
        # Homebrew / `nix build` is still compiled on every merge, and both
        # variants' cached dependencies reach a scope pull requests can restore
        # from. See .github/workflows/ci.yml for why that second part matters.
        claude-commander-ci = mkClaudeCommander {
          # Distinct pname so the two variants' store paths are tellable apart.
          pname = "claude-commander-ci";
          CARGO_PROFILE_RELEASE_LTO = "off";
          CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16";
        };
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
        # Rust std for the Apple targets cargokit maps Xcode's $PLATFORM_NAME /
        # $ARCHS pair onto — see the `Target.all` table in
        # client/rust_builder/cargokit/build_tool/lib/src/target.dart, which is
        # the authority on which triples can ever be asked for. Both macOS
        # arches are listed because `flutter build macos` is universal, so the
        # non-host one is still required; both simulator triples because the
        # simulator arch follows the Mac's, not the phone's.
        #
        # darwin-only, for the same reason the Android targets are unconditional:
        # a target you cannot link is dead weight. Linking these needs Xcode,
        # which Nix cannot package, so on Linux they would add closure for a
        # build that can never run. Adding them here is also precisely what
        # makes cargokit *see* them — the rustup shim below answers
        # `rustup target list --installed` by enumerating the toolchain sysroot.
        #
        # The host's own triple is subtracted rather than listed: `fenixStable`
        # (stable.toolchain) already carries rust-std for it, and `combine` joins
        # these into one derivation, so naming it again is a duplicate-file
        # collision. Unlike the Android targets — none of which can ever be the
        # host — aarch64-apple-darwin *is* the host on Apple silicon, so this
        # list is the first one that has to care.
        clientAppleTargets =
          clientPkgs.lib.optionals clientPkgs.stdenv.hostPlatform.isDarwin
            (map (t: fenix.packages.${system}.targets.${t}.stable.rust-std)
              (clientPkgs.lib.subtractLists
                [ clientPkgs.stdenv.hostPlatform.rust.rustcTarget ]
                [
                  "aarch64-apple-darwin"
                  "x86_64-apple-darwin"
                  "aarch64-apple-ios"
                  "aarch64-apple-ios-sim"
                  "x86_64-apple-ios"
                ]));
        clientRust = fenix.packages.${system}.combine ([
          fenixStable
          fenix.packages.${system}.targets.aarch64-linux-android.stable.rust-std
          fenix.packages.${system}.targets.armv7-linux-androideabi.stable.rust-std
          fenix.packages.${system}.targets.x86_64-linux-android.stable.rust-std
          fenix.packages.${system}.targets.i686-linux-android.stable.rust-std
        ] ++ clientAppleTargets);
        # Platform 36 is required by recent plugins (path_provider_android →
        # jni_flutter compileSdk 36); the Nix SDK is read-only so every needed
        # platform must be listed here rather than auto-installed by Gradle.
        # Ascending order — the last entry is what every Android subproject is
        # pinned to as its compile SDK (see ANDROID_COMPILE_SDK below).
        clientPlatformVersions = [ "34" "35" "36" ];
        clientAndroid = clientPkgs.androidenv.composeAndroidPackages {
          platformVersions = clientPlatformVersions;
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
        # Highest platform in `clientPlatformVersions`, derived rather than
        # repeated so bumping the list can't leave this behind. Every Android
        # subproject is pinned to it (see the ANDROID_COMPILE_SDK export) so no
        # plugin's own stale `compileSdkVersion` triggers a download into the
        # read-only store.
        clientCompileSdk = clientPkgs.lib.last clientPlatformVersions;
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
        # Host toolchain plus the Apple targets and none of the Android ones, for
        # the darwin CI job. `clientRust` would work, but its Android SDK/NDK is
        # gigabytes a macOS runner downloads and never opens — and the GitHub
        # Actions cache is a fixed budget shared with the Linux jobs, so paying
        # it here evicts theirs. `clientAppleTargets` is already darwin-guarded,
        # so on Linux this collapses to the host toolchain.
        clientAppleRust = fenix.packages.${system}.combine ([ fenixStable ] ++ clientAppleTargets);
        clientAppleRustupShim = mkRustupShim clientAppleRust;
        # `xcrun` on darwin is nixpkgs' xcbuild stub, and it locates an SDK only
        # via $DEVELOPER_DIR — which the stdenv sets, so it works for anything
        # inheriting the shell's environment. Dart's native-assets build hooks do
        # not: they run each `hooks/build.dart` under a filtered environment that
        # keeps PATH but drops DEVELOPER_DIR/SDKROOT. The stub then fails with
        # `error: unable to find sdk: 'macosx'`, that string lands in the
        # `-isysroot` the hook passes to clang, and every C/ObjC source it builds
        # fails on a missing `assert.h`/`Foundation.h`.
        #
        # This is not an Apple-build-only problem: `objective_c` arrives as a
        # transitive dependency, so its hook runs for the *host* build — i.e.
        # plain `flutter test` fails on macOS with no iOS/macOS target in sight.
        # Hence a shim rather than an exported variable: PATH is the only channel
        # that survives the filtering, so re-assert the SDK there.
        #
        # The other two branches are the same problem from the Apple-build side.
        # The Nix apple-sdk carries the macOS SDK *only*, so any request for
        # another Apple platform has to go to the host's Xcode: cc-rs asks
        # `xcrun --show-sdk-path --sdk iphoneos` for its sysroot, and that is
        # exactly where the `aarch64-apple-ios` staticlib build stops otherwise.
        # And because the stub only understands the Nix SDK's layout, a
        # DEVELOPER_DIR pointing at a real Xcode has to be handed off as well.
        #
        # DEVELOPER_DIR/SDKROOT are cleared before delegating: the host xcrun
        # reads them too, and the shell's values would send it back into the Nix
        # SDK it cannot make sense of.
        #
        # Everything else in `clientXcodeEnvScrub` goes for a different reason.
        # `xcodebuild` reads the *process environment* as build-setting
        # overrides: any variable whose name is a build setting silently beats
        # the value in the project. The Nix stdenv exports a whole toolchain
        # under exactly those names, so merely entering this shell rewrites the
        # Runner's build settings. `LD` is the one with teeth — Xcode links
        # through the clang driver, and `LD=ld` sends clang-driver flags to
        # ld64 instead, so the build dies in a pod with
        #   ld: -objc_abi_version '-Xlinker' not supported (expected 2)
        # which names neither Nix nor the variable that caused it. Both
        # deployment targets go too: Xcode sets whichever one its platform
        # needs, and the other one, left over from the shell, is "conflicting
        # deployment targets, both 'MACOSX_DEPLOYMENT_TARGET' and
        # 'IPHONEOS_DEPLOYMENT_TARGET' are present in environment".
        #
        # Scrubbed per delegation rather than at shell entry, because these are
        # also how a plain `cargo build` finds Nix's toolchain — only Xcode
        # mistakes them for build settings. The iOS deployment floor that
        # `cargo build` needs (see IPHONEOS_DEPLOYMENT_TARGET below) therefore
        # survives in the shell while staying out of Xcode's environment.
        clientXcodeEnvScrub = ''
          unset SDKROOT LD CC CXX AR AS NM RANLIB STRIP OBJCOPY OBJDUMP SIZE STRINGS
          unset MACOSX_DEPLOYMENT_TARGET IPHONEOS_DEPLOYMENT_TARGET
        '';
        # Apple's own xcrun, for cc-xcode-env's PATH. Pointing DEVELOPER_DIR at
        # the host Xcode is not enough on its own: `xcodebuild` lives in
        # Developer/usr/bin but `xcrun` does not — it is /usr/bin/xcrun — so a
        # lookup still lands on the shim below.
        #
        # Harmless on an Intel Mac, fatal on Apple silicon, which is why this only
        # ever failed in CI. `xcodeproj.dart` prefixes every Xcode command with
        # `/usr/bin/arch -arm64e` on a darwin_arm64 host to force them out of
        # Rosetta, and the shim is a shell script whose interpreter is Nix's bash:
        # plain arm64, no arm64e slice, so `arch` cannot exec it. Every
        # `xcrun xcodebuild -version` probe then fails, Flutter decides no Xcode is
        # installed, and the only thing it says is "Application not configured for
        # iOS" — naming neither xcrun, nor arch, nor a version.
        clientHostXcrun = clientPkgs.runCommand "cc-host-xcrun" { } ''
          mkdir -p $out/bin
          ln -s /usr/bin/xcrun $out/bin/xcrun
        '';
        # The same scrub, plus a host DEVELOPER_DIR, as a *command* — for anything
        # that talks to Xcode without going through `xcrun`. The shim below only
        # sees calls routed via xcrun, which is how nixpkgs' patched Flutter
        # behaves; a stock Flutter (Homebrew, fvm, or CI's Flutter action) runs
        # `xcodebuild` itself and never reaches it. Such a build inherits this
        # shell's DEVELOPER_DIR — the Nix apple-sdk, which is not an Xcode — plus
        # every Nix toolchain variable as a build-setting override.
        #
        # Nothing in the failure names any of that. `xcodebuild -showBuildSettings`
        # just returns nothing, so `productBundleIdentifier` is null and Flutter
        # exits with "Application not configured for iOS" — which reads as a
        # missing client/ios directory. That cost two full macOS CI runs to place,
        # hence a named wrapper rather than a line of exports at each call site.
        #
        #   cc-xcode-env flutter build ios --simulator
        clientXcodeEnv = clientPkgs.writeShellScriptBin "cc-xcode-env" ''
          set -eu
          hostDeveloperDir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p 2>/dev/null || true)"
          if [ ! -x "$hostDeveloperDir/usr/bin/xcodebuild" ]; then
            echo "cc-xcode-env: no Xcode selected — 'xcode-select -p' found no xcodebuild." >&2
            echo "  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
            exit 1
          fi
          # Ahead of the stdenv's xcbuild stub, which is what a bare `xcodebuild`
          # would otherwise resolve to.
          export DEVELOPER_DIR="$hostDeveloperDir"
          export PATH="${clientHostXcrun}/bin:$hostDeveloperDir/usr/bin:$PATH"
          ${clientXcodeEnvScrub}
          exec "$@"
        '';
        clientXcrunShim = clientPkgs.writeShellScriptBin "xcrun" ''
          hostXcrun() {
            unset DEVELOPER_DIR
            ${clientXcodeEnvScrub}
            exec /usr/bin/xcrun "$@"
          }
          for arg in "$@"; do
            case "$arg" in
              # Apple platforms the Nix apple-sdk does not carry.
              iphoneos|iphonesimulator|appletvos|appletvsimulator|watchos|watchsimulator|xros|xrsimulator)
                hostXcrun "$@" ;;
              # Tools the xcbuild stub does not carry at all. `xcodebuild` is
              # the one that matters: Flutter probes for Xcode with `xcrun
              # xcodebuild -version`, the stub answers "error: tool
              # 'xcodebuild' not found", and Flutter reports that as
              # "Application not configured for iOS" — which reads as a missing
              # client/ios directory rather than a shim that declined to
              # delegate. The rest are how Flutter enumerates and drives
              # simulators and devices.
              xcodebuild|xcdevice|simctl|devicectl|xcresulttool)
                hostXcrun "$@" ;;
            esac
          done
          if [ -z "''${DEVELOPER_DIR:-}" ]; then
            export DEVELOPER_DIR=${clientPkgs.apple-sdk}
          fi
          if [ "$DEVELOPER_DIR" != "${clientPkgs.apple-sdk}" ]; then
            ${clientXcodeEnvScrub}
            exec /usr/bin/xcrun "$@"
          fi
          exec ${clientPkgs.xcbuild.xcrun}/bin/xcrun "$@"
        '';
        # The Apple half of `devShells.client`'s hook, lifted out so the darwin
        # CI shell below can share it verbatim. Two shells that configure Xcode
        # differently is the failure this prevents: CI would go green against a
        # toolchain no maintainer actually uses. Everything Android or Linux
        # stays behind in `devShells.client`.
        clientAppleShellHook = ''
            # ---- Apple platforms: iOS + macOS desktop ----
            # Ahead of the stdenv's own xcbuild stub: nix develop puts stdenv's
            # bin dirs before this shell's inputs, so PATH order is the whole
            # point and buildInputs would be too late. See `clientXcrunShim`.
            export PATH="${clientXcodeEnv}/bin:${clientXcrunShim}/bin:$PATH"

            # Both are opt-in in Flutter's own config, and `flutter config` writes
            # to ~/.config/flutter_settings — i.e. it is per-user host state, not
            # something checking client/ios and client/macos into the repo turns
            # on. Without these, `flutter devices`/`flutter build` simply refuse
            # to see the two runners this shell exists to build.
            flutter config --enable-ios >/dev/null 2>&1 || true
            flutter config --enable-macos-desktop >/dev/null 2>&1 || true

            # Xcode is a host prerequisite (see buildInputs). Resolve it once,
            # here, and say so plainly at shell entry rather than letting the
            # first `flutter build ios` fail several minutes in with a CocoaPods
            # or codesign error.
            #
            # DEVELOPER_DIR has to come out of the environment for the probe to
            # mean anything: the stdenv points it at the Nix apple-sdk, and
            # `xcode-select -p` just echoes that back — so the check passes on a
            # machine with no Xcode at all. Test for `xcodebuild` under the
            # answer, since that is the tool whose absence actually bites.
            xcodeDir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p 2>/dev/null || true)"
            if [ ! -x "$xcodeDir/usr/bin/xcodebuild" ]; then
              echo "warning: no Xcode selected — 'xcode-select -p' found no xcodebuild."
              echo "  iOS/macOS builds need a full Xcode (not just the CLI tools):"
              echo "    sudo xcode-select -s /Applications/Xcode.app/Contents/Developer"
              echo "    sudo xcodebuild -runFirstLaunch"
            else
              # The iOS triples need Xcode's C compiler, not this shell's. cc-rs
              # falls back to plain `clang` off PATH for a target with no
              # CC_<triple>, and the Nix wrapper injects -mmacos-version-min,
              # which clang rejects outright next to -miphoneos-version-min. The
              # TLS stack makes that unavoidable rather than theoretical: rustls
              # resolves to aws-lc-rs here, so aws-lc-sys compiles C and arm64
              # assembly on the way to the staticlib.
              #
              # The darwin triples are redirected too, which they were not at
              # first: they are this host, so Nix's toolchain looked like the
              # right answer for them. It is not, under an Xcode-driven build.
              # cargokit runs cargo from an Xcode script phase, and
              # `clientXcodeEnvScrub` has to unset CC/CXX/AR/LD before delegating
              # (Xcode reads them as build settings), so a *host* build script —
              # objc-sys, libc, proc-macro2 — links through bare `cc`, i.e. Nix's
              # wrapper stripped of the environment it needs. On a maintainer's
              # Mac that still linked; on a GitHub arm64 runner it does not, and
              # `flutter build ios` dies with "linking with `cc` failed" naming
              # three build scripts and nothing else (Flutter mangles Xcode's
              # paths to `Pods/SEVERE:0:0`, so the real ld message never
              # surfaces). Build scripts are host artefacts, and
              # CARGO_TARGET_<HOST>_LINKER does govern their link while
              # cross-compiling — verified directly, not assumed.
              #
              # Resolved through xcrun, not by joining paths onto DEVELOPER_DIR:
              # only `xcodebuild` sits directly under it, while the compilers live
              # in the selected toolchain (Toolchains/XcodeDefault.xctoolchain),
              # whose name is not ours to assume.
              xcodeClang="$(unset DEVELOPER_DIR SDKROOT; /usr/bin/xcrun --find clang 2>/dev/null || true)"
              xcodeClangxx="$(unset DEVELOPER_DIR SDKROOT; /usr/bin/xcrun --find clang++ 2>/dev/null || true)"
              xcodeAr="$(unset DEVELOPER_DIR SDKROOT; /usr/bin/xcrun --find ar 2>/dev/null || true)"
              #
              # The linker has to move with the compiler. rustc links the cdylib
              # through Nix's cc wrapper otherwise, and that wrapper applies
              # NIX_LDFLAGS — which names Nix's *macOS* libiconv. ld refuses it in
              # an iOS link ("building for iOS Simulator, but linking in dylib
              # built for macOS"). Only the simulator triple that shares the
              # host's arch actually dies: on the device triple the arch mismatch
              # downgrades it to a warning and the dylib is skipped. And it is the
              # cdylib that fails, which cargokit does not even consume — it takes
              # the staticlib — but `crate-type` lists both, so one failing link
              # fails the build.
              #
              # Both darwin triples, not just this host's: a maintainer on an
              # Intel Mac cross-links the arm64 slice and hits the same wrapper.
              for appleTriple in aarch64_apple_ios aarch64_apple_ios_sim \
                                 x86_64_apple_ios aarch64_apple_darwin \
                                 x86_64_apple_darwin; do
                export "CC_$appleTriple=$xcodeClang"
                export "CXX_$appleTriple=$xcodeClangxx"
                export "AR_$appleTriple=$xcodeAr"
                export "CARGO_TARGET_$(echo "$appleTriple" | tr 'a-z' 'A-Z')_LINKER=$xcodeClang"
              done

              # Both halves have to agree on the minimum, and left alone they do
              # not: cc-rs takes the SDK's own default (26.2 today) while rustc
              # still defaults these triples to iOS 10, so the link asks libSystem
              # for symbols that only came with the compiler-rt of that era
              # (`___chkstk_darwin`, from aws-lc-sys' bignum assembly) and fails.
              #
              # Read out of the Runner instead of restated here, because a second
              # copy of the number is a copy that will drift — and drift lands as
              # that same link error rather than anything that names a version.
              # The Xcode project is the original: Flutter, CocoaPods and cargokit
              # all take the floor from it. Lowest wins if the configurations ever
              # disagree. Entering the shell from outside the repo therefore leaves
              # this unset, and a `cargo build` for an iOS triple fails on
              # `___chkstk_darwin` — cd to the repo root rather than exporting one
              # by hand.
              if [ -f client/ios/Runner.xcodeproj/project.pbxproj ]; then
                iosMin="$(sed -n 's/.*IPHONEOS_DEPLOYMENT_TARGET = \([0-9.]*\);.*/\1/p' \
                  client/ios/Runner.xcodeproj/project.pbxproj | sort -V | head -1)"
                if [ -n "$iosMin" ]; then
                  export IPHONEOS_DEPLOYMENT_TARGET="$iosMin"
                fi
              fi
            fi
        '';
      in
      {
        checks = {
          inherit claude-commander;
        };

        packages = {
          claude-commander = claude-commander;
          claude-commander-ci = claude-commander-ci;
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
            # Audio backends for the `audio` feature: ALSA + libpipewire. The
            # bindgenHook supplies LIBCLANG_PATH + BINDGEN_EXTRA_CLANG_ARGS for
            # the pipewire-sys/libspa-sys bindgen build (matches the package
            # build's nativeBuildInputs so dev shell and `nix build` agree).
            alsa-lib
            pipewire
            rustPlatform.bindgenHook
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
          # Linux and macOS, with each host's extra native stack appended
          # conditionally: GTK/X11 on Linux, CocoaPods (plus the Apple Rust
          # targets in `clientAppleTargets`) on macOS. Neither desktop's libs are
          # any use to the other — macOS renders through Cocoa and builds via
          # Xcode, which is a host prerequisite rather than a Nix input.
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
          ]) ++ clientPkgs.lib.optionals clientPkgs.stdenv.hostPlatform.isDarwin (with clientPkgs; [
            # Apple platforms (client/ios, client/macos). Flutter shells out to
            # `pod install` for both runners, because every plugin in this app
            # ships a podspec — rust_builder's included, and that one is what
            # runs cargokit's build_pod.sh to produce the Rust staticlib.
            #
            # Xcode is deliberately NOT here and cannot be: it is unpackageable
            # (licence + Apple's installer), so it stays a host prerequisite and
            # the shell inherits it from /usr/bin via xcode-select. That split is
            # why the Apple side can't be made hermetic the way Android is.
            cocoapods
            # Real (samba) rsync, ahead of the one in /usr/bin. macOS 15 replaced
            # that with openrsync, which accepts `--chmod` and then ignores it —
            # silently, so nothing in the build says so. Flutter unpacks its
            # engine framework with exactly that flag
            # (flutter_tools/.../darwin.dart: `--chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r`)
            # precisely so the copy is writable; ignored, the mode comes from the
            # source instead, and the source is the read-only Nix store. The build
            # then dies in `lipo`, thinning that framework in place, with a
            # "Permission denied" naming a temp file rather than the copy.
            rsync
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
            # Same problem, same shape, for compile SDKs: some plugins still
            # declare an ancient `compileSdkVersion` (irondash_engine_context,
            # via super_clipboard, asks for 31), and Gradle would try to install
            # that platform into the read-only Nix store. android/build.gradle.kts
            # reads this and pins every subproject to it — Android SDKs are
            # backward compatible for compilation, so raising a plugin's target
            # is safe and is what AGP's own "use the highest version" advice says.
            export ANDROID_COMPILE_SDK="${clientCompileSdk}"
            export JAVA_HOME="${clientPkgs.jdk17}"
            # Point Flutter at the Nix-provided SDK and silence analytics noise.
            flutter config --no-analytics >/dev/null 2>&1 || true
            flutter config --android-sdk "$ANDROID_SDK_ROOT" >/dev/null 2>&1 || true
${clientPkgs.lib.optionalString clientPkgs.stdenv.hostPlatform.isDarwin clientAppleShellHook}
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
            #
            # On failure this prints avdmanager's ACTUAL error. It used to swallow
            # stderr and print a guess about /dev/kvm, which was actively
            # misleading: the real failure here is `avdmanager` (a Java tool)
            # hitting a broken JDK — "libnio.so: undefined symbol: ipv6_available"
            # — while KVM is fine and the emulator itself, being native, boots
            # normally against an AVD created some other way. Never diagnose on the
            # user's behalf from an exit code alone.
            # Existence is checked on disk, not with `avdmanager list avd` —
            # that is the same Java tool, so when it is broken the check fails
            # too and creation is retried on every single shell entry even though
            # the AVD is right there. $ANDROID_AVD_HOME wins over the default when
            # set, matching the emulator's own lookup.
            avdHome="''${ANDROID_AVD_HOME:-$HOME/.android/avd}"
            if [ ! -f "$avdHome/cctest.ini" ]; then
              echo "Creating Android emulator AVD 'cctest' (android-35 google_apis x86_64)..."
              if avdCreateOut=$(avdmanager create avd -n cctest \
                   -k "system-images;android-35;google_apis;x86_64" \
                   --device pixel_6 --force 2>&1); then
                echo "AVD 'cctest' created."
              else
                echo "AVD creation failed. avdmanager said:"
                printf '%s\n' "$avdCreateOut" | sed 's/^/  /'
                echo "  (the emulator binary is native and can still run an AVD"
                echo "   created another way, e.g. by Android Studio.)"
              fi
            fi
            echo "entered claude-commander client dev shell (flutter + rust + android ndk${clientPkgs.lib.optionalString clientPkgs.stdenv.hostPlatform.isDarwin " + apple/cocoapods"})"
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

        # What `clientCi` is to the Linux job, this is to the darwin one: the
        # client shell with the parts a CI runner cannot use taken out. No
        # Android (see `clientAppleRust`), no GTK, no xvfb — just Flutter, the
        # Apple Rust targets, and CocoaPods driving the host's Xcode.
        #
        # The Apple configuration itself is `clientAppleShellHook`, shared with
        # `devShells.client` rather than restated, so CI cannot go green against
        # a differently-configured toolchain than the one maintainers use.
        #
        # Evaluates on Linux (as a bare Flutter shell) instead of being guarded
        # away, because `devShells` here is a flat attrset and a conditional
        # entry would need the whole set restructured; nothing consumes it there.
        devShells.clientApple = clientPkgs.mkShell {
          name = "claude-commander-client-apple";
          buildInputs = [
            clientAppleRust
            clientAppleRustupShim
          ] ++ (with clientPkgs; [
            flutter
            dart
            cmake
            ninja
            pkg-config
            clang
            llvmPackages.libclang
          ]) ++ clientPkgs.lib.optionals clientPkgs.stdenv.hostPlatform.isDarwin (with clientPkgs; [
            # Both are load-bearing for the same reasons they are in
            # `devShells.client`: every plugin ships a podspec, and macOS's
            # openrsync silently ignores the `--chmod` Flutter unpacks its
            # engine framework with.
            cocoapods
            rsync
          ]);

          LIBCLANG_PATH = "${clientPkgs.llvmPackages.libclang.lib}/lib";

          shellHook = ''
            flutter config --no-analytics >/dev/null 2>&1 || true
          '' + clientPkgs.lib.optionalString clientPkgs.stdenv.hostPlatform.isDarwin clientAppleShellHook;
        };
      }
    );
}
