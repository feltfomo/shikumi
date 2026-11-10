{ shikumi, ... }:
let
  helpers = import ./helpers.nix;
in
shikumi.project {
  project = {
    summary = helpers.required "summary" null;
    maintainers = [ "someone" ];
  };
}
