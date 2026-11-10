# a derivation the planted declarations read. `/bin/sh` is in every Nix build sandbox on Linux, so
# the check's control evaluation, which runs without the producer's refusals, can really build it.
# the harness copies this file beside each file that imports `./built.nix`, inside the repository,
# so the import itself is one the producer allows. the producer's evaluation is pure, which takes
# `builtins.currentSystem` away; the fallback lets the plant get past the system to the output read
# it plants, so that read is what's refused. the control isn't pure and builds for its own system.
script:
derivation {
  name = "shikumi-planted";
  system = builtins.currentSystem or "x86_64-linux";
  builder = "/bin/sh";
  args = [
    "-c"
    script
  ];
}
