{ shikumi, lib }:
shikumi.project (
  {
    project = {
      summary = "Split across files";
      maintainers = [ "someone" ];
    };
    commands = builtins.mapAttrs (_: description: { inherit description; }) {
      check = "Check without a command line";
    };
  }
  // import ./steps.nix { inherit lib; }
)
