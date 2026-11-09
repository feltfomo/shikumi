# a manifest written with the field names the layer table lists, so the layer has to read exactly
# those to find the project and the catalog.
let
  inherit (builtins.fromJSON (builtins.readFile ../../../layer.json)) reads;
in
{
  ${reads.projects}.app = "app";
  ${reads.catalog} = {
    ${reads.directory} = "catalog";
    ${reads.systems} = [ "x86_64-linux" ];
  };
}
