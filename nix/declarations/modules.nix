# the module adapter: modules for NixOS, Home Manager and flake-parts whose `shikumi.projects`
# option takes `shikumi.project` results, so a module system can carry declarations to whoever
# composes them. it only carries them. an entry has exactly one definition and comes back out as the
# very value that went in, with no merging, no priorities and no schema of its own; the checker reads
# it like any other declaration. each module takes the module system's own `lib`, not the one
# Shikumi runs on.
{ isProject }:
let
  option =
    lib:
    lib.mkOption {
      type = lib.types.attrsOf (
        lib.mkOptionType {
          name = "shikumiProject";
          description = "`shikumi.project` result";
          descriptionClass = "noun";
          # the mark the factory puts on, not the declaration's content.
          check = isProject;
          merge = lib.mergeOneOption;
        }
      );
      default = { };
      description = "Project declarations, each the result of `shikumi.project`, by project name.";
    };

  module =
    { lib, ... }:
    {
      options.shikumi.projects = option lib;
    };
in
{
  nixos = module;
  homeManager = module;
  # declarations at the top level, and per system for a declaration built from per-system values.
  # flake-parts exposes the per-system ones as `allSystems.<system>.shikumi.projects`.
  flakeParts =
    { lib, flake-parts-lib, ... }:
    {
      options = {
        shikumi.projects = option lib;
        perSystem = flake-parts-lib.mkPerSystemOption module;
      };
    };
}
