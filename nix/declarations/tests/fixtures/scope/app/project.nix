# each way a file reaches another, and what each sees: none of them may see a withheld builtin.
{ lib, shikumi }:
shikumi.project {
  project = {
    summary = "Looks for the withheld builtins";
    maintainers = [ "someone" ];
  };
  extensions.seen = {
    here = import ./seen.nix;
    builtinsImport = builtins.import ./seen.nix;
    scopedImport = scopedImport { } ./seen.nix;
    builtinsScopedImport = builtins.scopedImport { } ./seen.nix;
    scopedImportOwnBuiltins = scopedImport { inherit builtins; } ./seen.nix;
    evalModules = (lib.evalModules { modules = [ ./module.nix ]; }).config.seen;
    callPackageWith = lib.callPackageWith { } ./called.nix { };
    compose =
      (
        (shikumi.lib.compose {
          root = ../.;
          manifest = ../shikumi.nix;
        }).read
          "app/seen.nix"
      ).value.value;
  };
}
