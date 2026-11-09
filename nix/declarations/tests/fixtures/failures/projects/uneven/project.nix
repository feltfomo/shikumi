# fails in two places: where serialization reaches first, and in `dependsOn`, which is all a
# selection that reads only dependencies would reach. a focused composition has to report what the
# global one does.
{ shikumi, ... }:
shikumi.project {
  project = {
    summary = "Fails before its dependencies are read";
    maintainers = [ "someone" ];
    dependsOn = [ (builtins.head [ ]) ];
    after = throw "uneven fails here first";
  };
}
