let
  built = import ../../built.nix "echo planted > $out";
in
{
  project = {
    summary = if builtins.readFile built == "planted\n" then "Read a build's output" else "Unread";
    maintainers = [ "someone" ];
  };
}
