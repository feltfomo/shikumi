{ shikumi }:
shikumi.project {
  project = {
    summary = "Names things that don't exist";
    maintainers = [ "someone" ];
    dependsOn = [
      "sound"
      "nowhere"
    ];
  };
  steps.test = {
    description = "Test";
    exec = "test";
    after = [ "build" ];
  };
}
