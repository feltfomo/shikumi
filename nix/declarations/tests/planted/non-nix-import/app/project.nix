# a file in the project's directory that isn't a `.nix` file, which the keyed set leaves out.
{ shikumi }:
shikumi.project {
  project = {
    summary = import ./value.txt;
    maintainers = [ "someone" ];
  };
}
