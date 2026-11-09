let
  built = import ../../built.nix "echo planted > $out";
in
{
  project = {
    summary = "Hashed a build output to ${builtins.hashFile "sha256" built}";
    maintainers = [ "someone" ];
  };
}
