# a declaration split across files with plain `import` and `//`.
{ shikumi, ... }:
shikumi.project (
  {
    project = {
      summary = "The shared core library";
      maintainers = [ "core-team" ];
    };
    extensions.team = {
      owner = "platform";
      tier = "critical";
    };
  }
  // import ./steps.nix
)
