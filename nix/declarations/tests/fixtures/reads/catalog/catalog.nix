# a catalog written with the field names the layer table lists: its package, and an override whose
# build renames it, so the catalog's read shows each name was read.
_:
let
  inherit (builtins.fromJSON (builtins.readFile ../../../../layer.json)) reads;
in
{
  ${reads.packages}.hello = "hello";
  ${reads.overrides}.hello = {
    reason = "shows the override was read";
    release = "the test goes";
    ${reads.package} = _: prev: prev.hello.overrideAttrs { pname = "hello-read"; };
  };
}
