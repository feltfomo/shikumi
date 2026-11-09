{
  steps = {
    build = {
      description = "Compile the library";
      exec = "make";
    };
    test = {
      description = "Run the unit tests";
      exec = "make test";
      after = [ "build" ];
    };
  };
}
