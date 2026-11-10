# a store path with its string context taken off reads as a plain path. the producer refuses it: on
# Lix pure mode allows no path the evaluation wasn't given, and on CppNix and Determinate Nix the
# path isn't in the store the evaluation reads. the control writes the file into its own store and
# reads it.
{ shikumi }:
let
  stored = builtins.unsafeDiscardStringContext (builtins.toFile "shikumi-stripped" "planted");
in
shikumi.project {
  project = {
    summary = "Read ${builtins.readFile stored}";
    maintainers = [ "someone" ];
  };
}
