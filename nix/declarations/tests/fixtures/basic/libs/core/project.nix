# a declaration split across files with plain `import` and `//`.
{ shikumi, ... }:
shikumi.project (
  {
    project = {
      summary = "The shared core library";
      maintainers = [ "core-team" ];
    };
    toolchains.jdk = {
      description = "The JDK every JVM project builds and runs with";
      packages = [ "jdk" ];
      env.JAVA_HOME = {
        package = "jdk";
      };
    };
    extensions.team = {
      owner = "platform";
      tier = "critical";
    };
  }
  // import ./steps.nix
)
