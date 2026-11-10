# an overlay entry that renames the package it replaces, so the catalog shows it applied.
_: {
  packages.hello = "hello";
  overrides.hello = {
    reason = "shows the overlay applied";
    release = "the test that reads it goes";
    package = _: prev: prev.hello.overrideAttrs { pname = "hello-overridden"; };
  };
}
