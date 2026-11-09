# `shikumi.lib`, the declaration layer. the caller hands over nixpkgs' `lib`: nothing here reads
# `<nixpkgs>` or evaluates through a flake, so the one place that picks nixpkgs is whoever imports
# this file.
{ lib }:
let
  factory = import ./factory.nix;
  types = import ./vocabulary.nix;
  serialize = import ./serialize.nix { inherit lib; };

  shikumi = {
    inherit (factory) project;
    lib = {
      inherit compose types modules;
    };
  };

  compose = import ./compose.nix {
    inherit lib serialize types;
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
