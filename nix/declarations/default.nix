# `shikumi.lib`, the declaration layer. the caller hands over nixpkgs' `lib`: nothing here reads
# `<nixpkgs>` or evaluates through a flake, so the one place that picks nixpkgs is whoever imports
# this file.
{ lib }:
let
  # the names the layer and the registry's Rust share: each file's name and arguments, and the
  # frames the producer reads in a trace.
  layer = builtins.fromJSON (builtins.readFile ./layer.json);
  factory = import ./factory.nix;
  types = import ./vocabulary.nix;
  forced = import ./forced.nix;
  serialize = import ./serialize.nix { inherit lib forced; };

  shikumi = {
    inherit (factory) project;
    lib = {
      inherit compose types modules;
    };
  };

  # what a file the layer reads is offered: exactly the arguments the table names for it, so an
  # argument the table names and nothing here provides fails rather than being left out.
  offered = file: available: lib.genAttrs layer.${file}.arguments (name: available.${name});

  compose = import ./compose.nix {
    inherit
      lib
      serialize
      forced
      layer
      ;
    inherit (factory) isProject;
    # a declaration file is a function of these, or a value on its own.
    arguments = offered "declaration" { inherit shikumi lib; };
    manifestArguments = offered "manifest" { inherit types lib; };
  };

  modules = import ./modules.nix { inherit (factory) isProject; };
in
shikumi.lib
// {
  inherit (factory) project;
}
