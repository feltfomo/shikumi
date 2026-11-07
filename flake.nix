{
  description = "Shikumi, a build system and project registry that caches in the Nix store";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # the runner for the gate's Nix evaluation tests. no output uses it yet: it's locked now so the
    # first Nix logic worth asserting lands with its tests rather than with a new input.
    nix-unit = {
      url = "github:nix-community/nix-unit";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        # nix-unit's own formatter and CI matrix inputs; its package uses neither, and an empty
        # follows keeps them out of the lock so the Nix side stays nixpkgs and nix-unit.
        treefmt-nix.follows = "";
        nix-github-actions.follows = "";
      };
    };
  };

  outputs =
    { self, nixpkgs, ... }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];

      # only what Cargo reads, so editing Markdown or Nix doesn't rebuild the Rust checks.
      rustSource = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
          ./bench
          ./tools
        ];
      };

      nixSource = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.fileFilter (file: file.hasExt "nix") ./.;
      };

      perSystem =
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};

          cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };

          # runs Cargo over the workspace with the vendored dependencies and records success.
          cargoCheck =
            {
              name,
              tools,
              command,
            }:
            pkgs.stdenv.mkDerivation {
              inherit name cargoDeps;
              src = rustSource;
              nativeBuildInputs =
                (with pkgs; [
                  rustPlatform.cargoSetupHook
                  cargo
                  rustc
                ])
                ++ tools;
              buildPhase = ''
                runHook preBuild
                ${command}
                runHook postBuild
              '';
              installPhase = "touch $out";
            };

          # every binary in the workspace, from one build: separate packages over the same source
          # and dependencies would each compile the shared dependency graph again.
          binaries = pkgs.rustPlatform.buildRustPackage {
            pname = "shikumi-binaries";
            version = "0.1.0";
            src = rustSource;
            inherit cargoDeps;
            cargoBuildFlags = [
              "--package"
              "repo-check"
              "--package"
              "shikumi-conformance"
            ];
            # nextest owns the workspace tests; building them again here would only add time.
            doCheck = false;
          };

          # the same derivation under each binary's name, so `lib.getExe` finds the right one. meta
          # is set by update rather than `overrideAttrs`, which would make a second derivation.
          named =
            mainProgram:
            binaries
            // {
              meta = binaries.meta // {
                inherit mainProgram;
              };
            };
          repoCheck = named "repo-check";
          conformance = named "shikumi-conformance";

          # the tracked file set is exactly the flake source, so the checks list it from there.
          repoCheckRun =
            { name, subcommand }:
            pkgs.runCommand name { nativeBuildInputs = [ repoCheck ]; } ''
              cd ${self}
              find . -type f -printf '%P\0' | repo-check ${subcommand}
              touch $out
            '';
        in
        {
          packages = {
            repo-check = repoCheck;
            inherit conformance;
          };

          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              cargo-nextest
              deadnix
              rustfmt
              clippy
              nixfmt
              statix
              cargo
              rustc
            ];
          };

          # the conformance VMs need a Linux host with KVM; darwin evaluates the other checks alone.
          # the system string decides, not `pkgs.stdenv`, so listing the checks forces no package set.
          checks =
            lib.optionalAttrs (lib.hasSuffix "-linux" system) (
              import ./nix/conformance.nix { inherit pkgs conformance; }
            )
            // {
              # rustfmt reads sources only, so this check leaves out the vendored dependencies and
              # doesn't rebuild when they change.
              format =
                pkgs.runCommand "format"
                  {
                    nativeBuildInputs = with pkgs; [
                      rustfmt
                      nixfmt
                      cargo
                    ];
                  }
                  ''
                    cd ${rustSource}
                    cargo fmt --all --check
                    find ${nixSource} -name '*.nix' -exec nixfmt --check {} +
                    touch $out
                  '';
              nix-lint =
                pkgs.runCommand "nix-lint"
                  {
                    nativeBuildInputs = with pkgs; [
                      deadnix
                      statix
                    ];
                  }
                  ''
                    statix check ${nixSource}
                    deadnix --fail ${nixSource}
                    touch $out
                  '';
              clippy = cargoCheck {
                name = "clippy";
                tools = [ pkgs.clippy ];
                command = "cargo clippy --offline --workspace --all-targets -- --deny warnings";
              };
              nextest = cargoCheck {
                name = "nextest";
                tools = [ pkgs.cargo-nextest ];
                command = "cargo nextest run --offline --workspace --no-tests fail";
              };
              links = repoCheckRun {
                name = "links";
                subcommand = "links";
              };
              private-names = repoCheckRun {
                name = "private-names";
                subcommand = "names";
              };
            };

          formatter = pkgs.nixfmt;
        };

      bySystem = lib.genAttrs systems perSystem;
      outputOf = name: lib.mapAttrs (_: outputs: outputs.${name}) bySystem;
    in
    {
      packages = outputOf "packages";
      devShells = outputOf "devShells";
      checks = outputOf "checks";
      formatter = outputOf "formatter";
    };
}
