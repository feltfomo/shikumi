# Git's own files, beside the checkout's tracked ones.
{ shikumi }:
shikumi.project {
  project = {
    summary = "On ${builtins.readFile ../.git/HEAD}";
    maintainers = [ "someone" ];
  };
}
