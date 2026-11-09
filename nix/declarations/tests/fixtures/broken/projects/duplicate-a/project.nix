{ shikumi }:
shikumi.project {
  project = {
    summary = "Claims the same names as its twin";
    maintainers = [ "someone" ];
  };
  commands.fmt = {
    description = "Format";
    exec = "fmt";
  };
  services.db = {
    description = "A database";
    exec = "db";
    restart = "never";
  };
}
