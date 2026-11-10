{ shikumi }:
shikumi.project {
  project = {
    summary = "Names that aren't identifiers";
    maintainers = [ "someone" ];
    dependsOn = [ "Not Valid" ];
  };
  steps.Build = {
    description = "Build";
    exec = "build";
  };
}
