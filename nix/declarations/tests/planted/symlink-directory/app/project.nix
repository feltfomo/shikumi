# a directory reached through a tracked symlink, which the harness makes, to one no project lists.
{ shikumi }:
shikumi.project {
  project = {
    summary = import ./linked/lib.nix;
    maintainers = [ "someone" ];
  };
}
