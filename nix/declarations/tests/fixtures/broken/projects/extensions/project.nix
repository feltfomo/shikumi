{ shikumi }:
shikumi.project {
  project = {
    summary = "Extension values the schemas refuse";
    maintainers = [ "someone" ];
  };
  extensions = {
    team.owner = "Not An Identifier";
    meta.logo = ./project.nix;
    nobody = { };
    sneaky = "Kept at ${toString ./.}";
    layout.pages = [ "../outside/*.md" ];
  };
}
