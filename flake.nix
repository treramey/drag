{
  description = "Drag Tempo Cloud CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        workspace = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        drag = pkgs.rustPlatform.buildRustPackage {
          pname = "drag";
          version = workspace.workspace.package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--package" "drag-cli" ];
          cargoTestFlags = [ "--workspace" ];
          meta = with pkgs.lib; {
            description = "A fast Tempo.io Cloud command-line client";
            homepage = "https://github.com/treramey/drag";
            license = licenses.mit;
            mainProgram = "drag";
          };
        };
      in {
        packages.default = drag;
        packages.drag = drag;
        apps.default = flake-utils.lib.mkApp { drv = drag; };
        devShells.default = pkgs.mkShell { inputsFrom = [ drag ]; packages = with pkgs; [ cargo clippy rustc rustfmt ]; };
      });
}
