{ shikumi }:
shikumi.project {
  project = {
    summary = "Owns the JDK toolchain";
    maintainers = [ "someone" ];
  };
  toolchains.jdk = {
    description = "JDK 21";
    packages = [ "jdk" ];
    env.JAVA_HOME = {
      package = "jdk";
    };
  };
}
