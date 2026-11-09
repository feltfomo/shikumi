{ lib }:
{
  steps.build = {
    description = "Build";
    exec = "build";
    after = lib.optional true "Compile";
  };
}
