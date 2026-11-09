{
  imports = [ ./local.nix ];
  project = {
    summary = "The shared core library";
    maintainers = [ "core-team" ];
  };
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
  extensions.team = {
    owner = "platform";
    tier = "critical";
  };
}
