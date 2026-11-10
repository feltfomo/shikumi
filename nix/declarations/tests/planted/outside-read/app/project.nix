# a file outside the repository, which the build sandbox and every host have.
{ shikumi }:
shikumi.project {
  project = {
    summary = "Read ${builtins.readFile /etc/passwd}";
    maintainers = [ "someone" ];
  };
}
