# the worktree's directory name alone, which the schema's check for the whole location can't see.
{
  project = {
    summary = "Checked out as ${baseNameOf (toString ../.)}";
    maintainers = [ "someone" ];
  };
}
