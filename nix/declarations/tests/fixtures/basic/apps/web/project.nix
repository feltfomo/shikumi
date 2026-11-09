{ lib, ... }:
{
  imports = [ ./services.nix ];
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
    exec = lib.mkDefault "bundle --release";
  };
  commands.serve = {
    description = "Serve the site locally";
    exec = "serve --port 8080";
  };
  extensions.team.owner = "web";
}
