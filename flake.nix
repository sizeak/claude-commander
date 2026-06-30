{
  description = "Claude Commander - Terminal UI for managing Claude coding sessions";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, flake-utils }:
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
      }
    );
}
