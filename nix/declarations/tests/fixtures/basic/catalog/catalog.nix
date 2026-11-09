# every package the repository's shells and steps may use, each named once by its attribute path in
# the pinned nixpkgs.
_: {
  packages = {
    jdk = "jdk21";
    # Kotlin propagates the JDK it runs on, which the override below makes the one `jdk` names.
    kotlin = "kotlin";
    nodejs = "nodejs";
    mdbook = "mdbook";
  };
  overrides.kotlin = {
    reason = "Kotlin runs on the JDK every JVM project builds with, not the one nixpkgs picks for it";
    release = "nixpkgs' own choice of JDK for Kotlin is the repository's";
    package = final: prev: prev.kotlin.override { jre = final.jdk21; };
  };
}
