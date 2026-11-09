{ shikumi }:
let
  built = import ../../built.nix "echo planted > $out";
in
shikumi.project {
  project = {
    summary = if builtins.pathExists built then "Saw a build output exist" else "Saw nothing";
    maintainers = [ "someone" ];
  };
}
