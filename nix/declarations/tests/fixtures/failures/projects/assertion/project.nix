{ shikumi, ... }:
let
  replicas = 0;
in
assert replicas > 0;
shikumi.project {
  project = {
    summary = "Needs replicas";
    maintainers = [ "someone" ];
  };
}
