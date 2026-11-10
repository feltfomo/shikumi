{ shikumi }:
shikumi.project {
  project = {
    summary = "Picks packages the catalog doesn't name";
    maintainers = [ "someone" ];
  };
  packages = [
    "jdk"
    "nowhere"
  ];
  env.TOOL_HOME.package = "elsewhere";
}
