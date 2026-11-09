{ shikumi }:
let
  built = import ../../built.nix "echo '\"Imported a build output\"' > $out";
in
shikumi.project {
  project = {
    summary = import built;
    maintainers = [ "someone" ];
  };
}
