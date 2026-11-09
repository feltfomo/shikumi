{ shikumi }:
shikumi.project {
  project = {
    summary = "Restarts on a policy that doesn't exist";
    maintainers = [ "someone" ];
  };
  services.worker = {
    description = "A worker";
    exec = "worker";
    restart = "sometimes";
  };
}
