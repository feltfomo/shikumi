# what `shikumi registry` evaluates through the host's `nix-instantiate`. every argument arrives as
# a string, so nothing the caller passes is parsed as Nix. `nixpkgs` is the source tree whose `lib`
# the layer runs on, and the only nixpkgs the evaluation sees.
{
  nixpkgs,
  root,
  focus ? null,
}:
let
  shikumi = import ./. { lib = import (/. + nixpkgs + "/lib"); };
  checkout = /. + root;
  inherit
    (shikumi.compose {
      root = checkout;
      manifest = checkout + "/shikumi.nix";
      inherit focus;
    })
    document
    ;
in
# the schema refuses every string that carries a store reference, so one reaching here means the
# layer broke its own contract, not that a declaration is wrong.
if builtins.hasContext (builtins.toJSON document) then
  throw "shikumi: the registry document carries a store reference, which the declaration schema should have refused"
else
  document
