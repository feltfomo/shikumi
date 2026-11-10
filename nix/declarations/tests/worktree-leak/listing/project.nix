# a listing of the repository's directory, which a checkout's untracked files would change.
{ shikumi }:
shikumi.project {
  project = {
    summary = "Beside ${builtins.concatStringsSep ", " (builtins.attrNames (builtins.readDir ../.))}";
    maintainers = [ "someone" ];
  };
}
