# a directory outside the repository, with no flake in it, read through `findFile` with a
# lookup-path entry that names it by a `flake:path:` reference.
{ shikumi }:
shikumi.project {
  project = {
    summary = builtins.readFile (
      builtins.findFile [
        {
          prefix = "v";
          path = "flake:path:@outside@/lookup-dir";
        }
      ] "v/secret.txt"
    );
    maintainers = [ "someone" ];
  };
}
