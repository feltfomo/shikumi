# a value whose attributes come every way a declaration's can: written in place, in a dotted path,
# from another file through `//`, and built by functions, one of them nixpkgs'.
{ lib }:
{
  project = {
    summary = "In place";
    tags = [
      "a"
      "b"
    ];
  };
  steps.build.description = "Dotted";
  mapped = builtins.mapAttrs (_: value: value) { inner = 1; };
  generated = lib.genAttrs [ "inner" ] (_: 2);
}
// import ./part.nix
