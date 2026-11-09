# the first byte of a two-byte character, which no text can hold on its own.
{ shikumi, ... }:
shikumi.project {
  project = {
    summary = builtins.substring 0 1 "été";
    maintainers = [ "someone" ];
  };
}
