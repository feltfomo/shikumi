let
  built = derivation {
    name = "fixture";
    system = "x86_64-linux";
    builder = "/bin/sh";
  };
in
{
  project = {
    summary = "Built by ${built}";
    maintainers = [ "someone" ];
  };
}
