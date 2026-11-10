{ shikumi }:
shikumi.project {
  project = {
    summary = "Depends on a project that depends on it";
    maintainers = [ "someone" ];
    dependsOn = [
      "sound"
      "cycle-b"
    ];
  };
}
