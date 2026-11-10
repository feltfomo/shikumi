_: {
  packages = {
    jdk = "jdk21";
    # Kotlin on JDK 25, which it propagates onto the path of whatever picks it.
    kotlin = "kotlin";
    # no package in the pinned nixpkgs.
    missing = "no-such-package";
  };
  overrides = {
    kotlin = {
      reason = "a Kotlin built on JDK 25";
      release = "the repository moves to JDK 25";
      package = final: prev: prev.kotlin.override { jre = final.jdk25; };
    };
    hello = {
      release = "never";
      package = _: prev: prev.hello;
    };
    cowsay = {
      reason = "a newer cowsay";
      package = _: prev: prev.cowsay;
    };
  };
  overlays = [ (_: _: { }) ];
}
