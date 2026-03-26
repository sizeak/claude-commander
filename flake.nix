{
  description = "Claude Commander - Terminal UI for managing Claude coding sessions";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        claude-commander = pkgs.rustPlatform.buildRustPackage {
          pname = "claude-commander";
          version = "0.1.0";

          src = pkgs.lib.cleanSource ./.;

          cargoHash = "sha256-fV72COYJAgu7OUh1dQRzKK7C8RaolKdbkV9k9AIFpZg=";

          # Some tests require a real git repo, which isn't available in the Nix sandbox
          doCheck = false;

          nativeBuildInputs = with pkgs; [
            pkg-config
            makeWrapper
          ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.apple-sdk_15
          ];

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
        };
      in
      {
        packages = {
          claude-commander = claude-commander;
          default = claude-commander;
        };

        apps.default = {
          type = "app";
          program = "${claude-commander}/bin/claude-commander";
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ claude-commander ];
          packages = with pkgs; [
            cargo
            rustc
            rust-analyzer
            clippy
            rustfmt
            tmux
            git
          ];
        };
      }
    );
}
