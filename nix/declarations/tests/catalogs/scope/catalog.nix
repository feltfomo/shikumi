# an override whose build comes from a file nixpkgs' `callPackage` imports, with nixpkgs' own
# `import`; its name says whether that file saw a withheld builtin.
_: {
  packages.hello = "hello";
  overrides.hello = {
    reason = "shows what a file nixpkgs imports sees";
    release = "the test that reads it goes";
    package = final: prev: final.callPackage ./reached.nix { inherit (prev) hello; };
  };
}
