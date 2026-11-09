{ shikumi }:
shikumi.project {
  project = {
    summary = "Sets the shell's path itself";
    maintainers = [ "someone" ];
  };
  # the shell's path, which the packages its sources pick make.
  env.PATH = "/usr/bin";
}
