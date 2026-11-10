{ shikumi, ... }:
let
  name = "service";
in
shikumi.project {
  project = {
    summary = name "called as a function";
    maintainers = [ "someone" ];
  };
}
