# `shikumi.lib`, the declaration layer. the caller hands over nixpkgs' `lib`: nothing here reads
# `<nixpkgs>` or evaluates through a flake, so the one place that picks nixpkgs is whoever imports
# this file.
{ lib }:
let
  schema = import ./schema.nix { inherit lib; };
in
{
  compose = import ./compose.nix { inherit lib schema; };
  # the vocabulary a repository writes its extension schemas in.
  inherit (schema) types;
}
