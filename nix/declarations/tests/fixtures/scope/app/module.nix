# a module the module system imports, with nixpkgs' own `import`.
{ lib, ... }:
{
  options.seen = lib.mkOption { };
  config.seen = import ./seen.nix;
}
