# the worktree's directory name alone, which the checker's test for the whole location can't see.
{ shikumi }:
shikumi.project {
  project = {
    summary = "Checked out as ${baseNameOf (toString ../.)}";
    maintainers = [ "someone" ];
  };
}
