# another repository's keyed copy, which the harness puts in Shikumi's store first, named by its
# exact NAR hash. an evaluation's own store registers only what it was given, so the copy isn't
# there to enter. the control adds the same tree to its own store and reads it.
{ shikumi }:
shikumi.project {
  project = {
    summary = builtins.readFile (
      builtins.path {
        path = /no/such/path;
        name = "shikumi-keyed";
        sha256 = "sha256-1Ro2pWbI/vJ0PvaHgENObLOyYcvmTaW+vmi9e5oHzJs=";
      }
      + "/app/project.nix"
    );
    maintainers = [ "someone" ];
  };
}
