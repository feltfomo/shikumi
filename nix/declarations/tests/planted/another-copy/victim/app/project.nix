# a repository the harness evaluates before the case, so its keyed copy is in Shikumi's store when
# the case's declaration names that copy by its NAR hash.
{ shikumi }:
shikumi.project {
  project = {
    summary = "Another repository's declaration";
    maintainers = [ "someone" ];
  };
}
