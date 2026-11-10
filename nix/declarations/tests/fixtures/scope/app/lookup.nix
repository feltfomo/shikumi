# a lookup-path entry that names a flake, which reaches the fetchers `fetchTree` stands for.
builtins.findFile [
  {
    prefix = "v";
    path = "flake:path:/nowhere";
  }
] "v/secret.txt"
