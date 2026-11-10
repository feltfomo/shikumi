# a catalog package whose output only a build could write. the toolchain context has to give the
# path a build of it writes, on each system, and build nothing; the control builds it for its own
# system from this same entry. the system comes from the package set the overlay is applied to, so
# the producer's pure evaluation needs no `builtins.currentSystem`.
_: {
  packages.planted = "shikumi-planted";
  overrides.shikumi-planted = {
    reason = "The package the toolchain context resolves without building";
    release = "Never";
    package =
      final: _:
      derivation {
        name = "shikumi-planted";
        inherit (final.stdenv.hostPlatform) system;
        builder = "/bin/sh";
        args = [
          "-c"
          "echo planted > $out"
        ];
      };
  };
}
