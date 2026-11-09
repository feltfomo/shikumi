# the catalog is the one place the layer evaluates a package set, so it's held to the same refusal
# as a declaration: an overlay entry whose reason reads a build's output.
_:
let
  built = import ../../built.nix "echo planted > $out";
in
{
  overrides.hello = {
    reason = builtins.readFile built;
    release = "never";
    package = _: prev: prev.hello;
  };
}
