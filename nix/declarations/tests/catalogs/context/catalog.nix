# a catalog whose toolchain context holds a package with a path and one whose path can't be
# computed, as a package nixpkgs refuses to evaluate can't.
_: {
  packages = {
    hello = "hello";
    refused = "shikumi-refused";
  };
  overrides.shikumi-refused = {
    reason = "A package whose output path can't be computed";
    release = "The test goes";
    package = _: prev: prev.hello // { outPath = throw "refused"; };
  };
}
