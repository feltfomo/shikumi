# another Git repository, read through `findFile` with a lookup-path entry that names it as a flake
# at a locked revision: Determinate Nix's flake fetcher resolves the entry in pure and restricted
# mode. the harness commits the case's `outside/` there, at a fixed date.
{ shikumi }:
shikumi.project {
  project = {
    summary = builtins.readFile (
      builtins.findFile [
        {
          prefix = "v";
          path = "flake:git+file://@outside@/lookup-git?rev=9a2ba2a526f967fff74ddb99ea7d4b85753b1a5b";
        }
      ] "v/secret.txt"
    );
    maintainers = [ "someone" ];
  };
}
