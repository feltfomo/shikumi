# depends on projects whose declarations fail, one of them in its dependencies too.
{ shikumi, ... }:
shikumi.project {
  project = {
    summary = "Depends on broken projects";
    maintainers = [ "someone" ];
    dependsOn = [
      "thrown"
      "undefined"
      "uneven"
    ];
  };
}
