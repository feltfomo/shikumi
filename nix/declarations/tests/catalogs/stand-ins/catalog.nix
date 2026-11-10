# packages built without nixpkgs' builders, added by the overlay, so what propagation reads is
# exactly what's written here.
_:
let
  package =
    name: attributes:
    derivation (
      {
        inherit name;
        system = "x86_64-linux";
        builder = "/bin/sh";
      }
      // attributes
    );
  base = package "base-1.0" { };
  inner = package "inner-2.0" { propagatedBuildInputs = [ base ]; };
  tool = package "tool-3.0" { meta.mainProgram = "tool"; };
  added = value: {
    reason = "a stand-in for the test";
    release = "the test goes";
    package = _: _: value;
  };
in
{
  packages = {
    outer = "outer";
    inner = "inner";
    named = "named";
    unversioned = "unversioned";
  };
  overrides = {
    # nested lists are flattened, and nulls and text are no packages.
    outer = added (
      package "outer-1.0" {
        propagatedBuildInputs = [
          null
          "text"
          [ inner ]
        ];
        propagatedNativeBuildInputs = [ tool ];
      }
    );
    # reached again through `outer`, which the graph holds once.
    inner = added inner;
    named = added (
      package "named-0.1" {
        pname = "custom";
        version = "9";
      }
    );
    unversioned = added (package "plain" { });
  };
}
