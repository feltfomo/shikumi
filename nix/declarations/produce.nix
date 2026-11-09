# what the producer evaluates through the host's `nix-instantiate`. every argument arrives as a
# string, so nothing the caller passes is parsed as Nix. `nixpkgs` is the source tree whose `lib`
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
# serialization turns every string's context into a flag and every path into plain text, so the
# document carries no store reference; the `declarations` check holds that over a whole composed
# document.
document
