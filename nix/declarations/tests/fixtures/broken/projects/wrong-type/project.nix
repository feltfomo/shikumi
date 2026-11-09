{ shikumi }:
shikumi.project {
  project = {
    summary = "Maintainers is a string and steps a list";
    maintainers = "someone";
  };
  steps = [ ];
  commands.run = {
    description = "Runs a function instead of a command line";
    exec = command: command;
  };
}
