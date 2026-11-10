# `shikumi.lib`, the declaration layer. the caller hands over the nixpkgs source tree, the one
# place that picks the nixpkgs declarations run on and the catalog reads.
{ nixpkgs }: (import ./layer.nix { inherit nixpkgs; }).lib
