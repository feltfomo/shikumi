# a search-path lookup. the control finds the entry the harness puts on its search path.
{ shikumi }:
shikumi.project {
  project = {
    summary = import <shikumi-planted-channel>;
    maintainers = [ "someone" ];
  };
}
