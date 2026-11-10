# a project that picks the catalog's package.
{ shikumi }:
shikumi.project {
  project = {
    summary = "Picks the located package";
    maintainers = [ "someone" ];
  };
  packages = [ "located" ];
}
