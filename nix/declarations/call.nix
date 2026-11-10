# calls a file's function with the arguments it names, or with all of them when it names none. a
# function that requires an argument nobody gives it stays a function, which the checker reports,
# rather than failing evaluation for every other declaration too. the manifest, every declaration
# and the catalog are read through here, each with the arguments it's offered.
{ lib, forced }:
available: imported:
let
  value = forced imported;
  named = builtins.functionArgs value;
  unmet = lib.filterAttrs (name: optional: !optional && !(available ? ${name})) named;
in
if !builtins.isFunction value || unmet != { } then
  value
else
  forced (value (if named == { } then available else builtins.intersectAttrs named available))
