# Determinate Nix 3.24.2, which nixpkgs doesn't carry, built from its published source tag with
# nixpkgs' own modular Nix packaging. the tag is CppNix 2.35.3 underneath (`.version` in the tree),
# the same base as `nixVersions.nix_2_35`, so nixpkgs' component definitions fit it once the
# dependencies Determinate pins differently are overridden. Determinate publishes no standalone
# binary for 3.24.2: its only artifact is a closure linked against its own store paths, embedded in
# the installer, which can't become a derivation output.
#
# the overrides follow `packaging/dependencies.nix` in the tag. what's left out, the wasm builtins,
# sentry and OpenTelemetry, doesn't touch the store, the daemon or the derivation code the
# conformance checks exercise.
{ pkgs }:
let
  inherit (pkgs) lib;

  version = "3.24.2";

  src = pkgs.fetchFromGitHub {
    owner = "DeterminateSystems";
    repo = "nix-src";
    tag = "v${version}";
    hash = "sha256-QY69Nw0WhXSvT2n2ldI5gt0h4ofbAVoi7EYGJGyNLl0=";
  };

  components =
    pkgs.nixDependencies.callPackage
      "${pkgs.path}/pkgs/tools/package-management/nix/modular/packages.nix"
      {
        inherit version src;
        teams = [ ];
        otherSplices = pkgs.generateSplicesForMkScope [
          "nixVersions"
          "nixComponents_2_35"
        ];
      };

  # nixpkgs' meson hook passes `-Dauto_features=enabled`, so every optional feature whose
  # dependency nixpkgs' component definitions don't supply has to be turned off by name.
  withMeson =
    {
      flags ? [ ],
      inputs ? [ ],
    }:
    drv:
    drv.overrideAttrs (prev: {
      mesonFlags = (prev.mesonFlags or [ ]) ++ flags;
      buildInputs = (prev.buildInputs or [ ]) ++ inputs;
    });

  determinate = components.overrideScope (
    _: prev: {
      # Determinate's garbage collector fork; eval-gc.cc calls `GC_get_approx_sp`, which upstream
      # bdwgc lacks.
      boehmgc = pkgs.nixDependencies.boehmgc.overrideAttrs (attrs: {
        src = pkgs.fetchFromGitHub {
          owner = "DeterminateSystems";
          repo = "bdwgc";
          rev = "8505c09cafce7bd2cffc421fd94b79390f37e428";
          hash = "sha256-ZXxPgcmKohIJv8sYVHJaUV36/OCfTGQh+N7VfrPHVAM=";
        };
        nativeBuildInputs = (attrs.nativeBuildInputs or [ ]) ++ [ pkgs.buildPackages.autoreconfHook ];
        env = (attrs.env or { }) // {
          NIX_CFLAGS_COMPILE = toString [
            "-DINITIAL_MARK_STACK_SIZE=1048576"
            "-DGC_MANY_BLOCKS_DEFAULT=64"
            "-DGC_TINY_FREELISTS=96"
            "-DNO_BLACK_LISTING"
          ];
        };
      });
      # libgit2 2.0.0-rc.1 with the tag's patches; libfetchers calls
      # `git_config_backend_from_values`, which the nixpkgs release lacks. the patches are read from
      # the fetched source at build time, which is a build input, not import-from-derivation.
      libgit2 = pkgs.libgit2.overrideAttrs (attrs: {
        version = "2.0.0-rc.1";
        src = pkgs.fetchFromGitHub {
          owner = "libgit2";
          repo = "libgit2";
          rev = "ae45d0d168f7e8dbfdb8c623589cb51caac96ab3";
          hash = "sha256-3sbqHm37SOwBeFgtjI2DLN6kx1F7G2N1m6rRIkqDXNI=";
        };
        # applied in the order the tag lists them.
        patches = (attrs.patches or [ ]) ++ [
          "${src}/packaging/patches/0002-memory-config.patch"
          "${src}/packaging/patches/0003-packbuilder-correct-config.patch"
          "${src}/packaging/patches/libgit2-packbuilder-dont-fail-on-thread-create-error.patch"
        ];
        meta = removeAttrs attrs.meta [ "changelog" ];
      });
      nix-store = withMeson { flags = [ "-Dwasm=disabled" ]; } prev.nix-store;
      nix-expr = withMeson { flags = [ "-Dwasm=disabled" ]; } prev.nix-expr;
      nix-fetchers = withMeson { inputs = [ pkgs.zstd ]; } prev.nix-fetchers;
      nix-cli = withMeson {
        flags = [
          "-Dsentry=disabled"
          "-Dotel=disabled"
        ];
        inputs = [ pkgs.libmicrohttpd ];
      } prev.nix-cli;
    }
  );
in
# `nix-cli` carries the commands, the daemon and its systemd units, which is all `nix.package`
# needs. the manual and functional tests in nixpkgs' full package don't fit Determinate's tree.
lib.addMetaAttrs { mainProgram = "nix"; } determinate.nix-cli
