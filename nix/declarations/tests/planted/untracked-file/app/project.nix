# a file beside the declaration that Git doesn't track, which the harness leaves out of the index.
{ shikumi }:
shikumi.project {
  project = {
    summary = import ./untracked.nix;
    maintainers = [ "someone" ];
  };
}
