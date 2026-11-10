{ shikumi }:
shikumi.project {
  project = {
    summary = "Orders its steps in a loop";
    maintainers = [ "someone" ];
  };
  steps = {
    build = {
      description = "Build";
      exec = "build";
      after = [ "test" ];
    };
    test = {
      description = "Test";
      exec = "test";
      after = [ "build" ];
    };
    lint = {
      description = "Lint";
      exec = "lint";
    };
  };
}
