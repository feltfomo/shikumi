# the host's system, which pure evaluation takes away.
{ shikumi }:
shikumi.project {
  project = {
    summary = "Produced on ${builtins.currentSystem}";
    maintainers = [ "someone" ];
  };
}
