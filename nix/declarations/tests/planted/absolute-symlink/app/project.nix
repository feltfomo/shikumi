# a tracked symlink to a file outside the commit, by its absolute path.
{ shikumi }:
shikumi.project {
  project = {
    summary = import ./outside.nix;
    maintainers = [ "someone" ];
  };
}
