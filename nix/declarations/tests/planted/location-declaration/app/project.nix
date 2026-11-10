# values built from where the evaluator reads the copy, none of which holds the location itself:
# its length, part of a hash of it, and the length of a source position's file. on an evaluator
# that reads the copy where it lies under the cache directory, each differs with that directory.
{ shikumi }:
let
  here = toString ./.;
in
shikumi.project {
  project = {
    summary = "Built from where the copy lies";
    maintainers = [ "someone" ];
  };
  env = {
    LENGTH = toString (builtins.stringLength here);
    HASH = builtins.substring 0 16 (builtins.hashString "sha256" here);
    POSITION = toString (builtins.stringLength (builtins.unsafeGetAttrPos "a" { a = 1; }).file);
  };
}
