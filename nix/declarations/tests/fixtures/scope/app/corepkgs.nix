# the file nixpkgs' bootstrap imports through the lookup path, which every implementation resolves
# without one.
builtins.isPath <nix/fetchurl.nix>
