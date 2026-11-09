# `shikumi.lib`, the declaration layer. the caller hands over nixpkgs' `lib`: nothing here reads
# `<nixpkgs>` or evaluates through a flake, so the one place that picks nixpkgs is whoever imports
# this file.
{ lib }:
let
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

  compose = import ./compose.nix {
    inherit
      lib
      serialize
      types
      forced
      ;
    inherit (factory) isProject;
    # a declaration file is a function of these, or a value on its own.
    arguments = { inherit shikumi lib; };
  };

  modules = import ./modules.nix { inherit (factory) isProject; };
in
shikumi.lib
// {
  inherit (factory) project;
}
