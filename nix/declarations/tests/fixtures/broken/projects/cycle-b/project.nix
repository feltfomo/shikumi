{ shikumi }:
shikumi.project {
  project = {
    summary = "Depends back on the project that depends on it";
    maintainers = [ "someone" ];
    dependsOn = [ "cycle-a" ];
  };
}
