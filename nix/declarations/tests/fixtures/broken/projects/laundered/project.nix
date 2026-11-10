# `match` hands back the copied path's text without the store context that would mark it.
{ shikumi }:
let
  copied = builtins.head (builtins.match "(.*)" "${./.}");
in
shikumi.project {
  project = {
    summary = "Copied to ${copied}";
    maintainers = [ "someone" ];
  };
}
