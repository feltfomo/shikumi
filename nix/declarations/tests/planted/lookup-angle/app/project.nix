# the same directory read through `<...>`, which looks the name up in the nearest `__nixPath`: one
# the declaration binds itself, so no builtin is named at all.
{ shikumi }:
let
  # `<...>` reads this binding by name, which deadnix can't see.
  # deadnix: skip
  __nixPath = [
    {
      prefix = "v";
      path = "flake:path:@outside@/lookup-angle";
    }
  ];
in
shikumi.project {
  project = {
    summary = builtins.readFile <v/secret.txt>;
    maintainers = [ "someone" ];
  };
}
