# a derivation the planted declarations read. `/bin/sh` is in every Nix build sandbox on Linux, so
# the check's control evaluation, which runs without the producer's refusals, can really build it.
script:
derivation {
  name = "shikumi-planted";
  system = builtins.currentSystem;
  builder = "/bin/sh";
  args = [
    "-c"
    script
  ];
}
