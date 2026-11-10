# the clock, which pure evaluation takes away.
{ shikumi }:
shikumi.project {
  project = {
    summary = "Produced at ${toString builtins.currentTime}";
    maintainers = [ "someone" ];
  };
}
