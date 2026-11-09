{ shikumi }:
shikumi.project {
  project = {
    summary = "Uses the JDK toolchain and Kotlin, which brings another JDK";
    maintainers = [ "someone" ];
    imports = {
      jdk = "jdk-owner";
      rust = "jdk-owner";
    };
  };
  packages = [ "kotlin" ];
  env.JAVA_HOME = "/opt/jdk";
}
