{ shikumi, lib }:
let
  services = import ./services.nix;
in
shikumi.project (
  {
    project = {
      summary = "The web front end";
      maintainers = [
        "web-team"
        "design"
      ];
      dependsOn = [ "core" ];
    };
    steps.bundle = {
      description = "Bundle the site";
      exec = "bundle --release";
    };
    # the local server talks to the development database, so it's declared only beside it.
    commands = lib.optionalAttrs (services.services ? database) {
      serve = {
        description = "Serve the site locally";
        exec = "serve --port 8080";
      };
    };
    extensions.team.owner = "web";
  }
  // services
)
