# the declaration layer, wired together once: `lib`, the `shikumi.lib` that `default.nix` hands to
# whoever imports the layer, and what only the producer asks for: the toolchain context, and the
# table the layer shares with the registry's Rust. the caller
# hands over the nixpkgs source tree: nothing here reads `<nixpkgs>` or evaluates through a flake,
# so the one place that picks nixpkgs is whoever imports the layer. declarations run on its `lib`,
# and the catalog reads its package set.
{ nixpkgs }:
let
  # the names the layer and the registry's Rust share: each file's name and arguments, the frames
  # the producer reads in a trace, and the builtins the scope withholds.
  layer = builtins.fromJSON (builtins.readFile ./layer.json);
  # every user file, and nixpkgs with it, is imported in this scope, never with `import`.
  scope = import ./scope.nix {
    inherit nixpkgs;
    inherit (layer) withheld;
  };
  lib = scope.import (nixpkgs + "/lib");
  factory = import ./factory.nix;
  types = import ./vocabulary.nix;
  forced = import ./forced.nix;
  serialize = import ./serialize.nix { inherit lib forced; };
  call = import ./call.nix { inherit lib forced; };

  shikumi = {
    inherit (factory) project;
    lib = {
      inherit
        compose
        types
        modules
        ;
      catalog = catalogs.read;
    };
  };

  # what a file the layer reads is offered: exactly the arguments the table names for it, so an
  # argument the table names and nothing here provides fails rather than being left out.
  offered = file: available: lib.genAttrs layer.${file}.arguments (name: available.${name});

  compose = import ./compose.nix {
    inherit
      serialize
      forced
      call
      layer
      ;
    inherit (scope) importIn;
    inherit (factory) isProject;
    # a declaration file is a function of these, or a value on its own.
    arguments = offered "declaration" { inherit shikumi lib; };
    manifestArguments = offered "manifest" { inherit types lib; };
  };

  catalogs = import ./catalog.nix {
    inherit
      lib
      forced
      serialize
      call
      nixpkgs
      ;
    inherit (scope) importIn;
    # what the catalog file is called with: no package set, which only Shikumi resolves.
    arguments = offered "catalog" { inherit lib; };
  };

  modules = import ./modules.nix { inherit (factory) isProject; };
in
{
  lib = shikumi.lib // {
    inherit (factory) project;
  };
  inherit (catalogs) context;
  inherit layer;
}
