let
  built = import ../../built.nix "echo '\"Imported a build output\"' > $out";
in
{
  project = {
    summary = import built;
    maintainers = [ "someone" ];
  };
}
