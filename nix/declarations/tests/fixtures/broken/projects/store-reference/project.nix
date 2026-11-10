{ shikumi }:
let
  built = derivation {
    name = "fixture";
    system = "x86_64-linux";
    builder = "/bin/sh";
  };
in
shikumi.project {
  project = {
    summary = "Built by ${built}";
    maintainers = [ "someone" ];
  };
}
