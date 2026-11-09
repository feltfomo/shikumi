{
  project = {
    summary = "Sets options Shikumi doesn't have";
    maintainers = [ "someone" ];
    owner = "someone";
  };
  tasks.build = { };
  steps.lint = {
    description = "Lint";
    exec = "lint";
    cmd = "lint";
  };
}
