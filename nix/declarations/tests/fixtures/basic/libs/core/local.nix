# an option the declaration keeps to itself: Shikumi neither reports nor carries it.
{ lib, ... }:
{
  options.localNote = lib.mkOption { type = lib.types.str; };
  config.localNote = "kept to itself";
}
