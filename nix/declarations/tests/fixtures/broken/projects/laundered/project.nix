# `match` hands back the copied path's text without the store context that would mark it.
let
  copied = builtins.head (builtins.match "(.*)" "${./.}");
in
{
  project = {
    summary = "Copied to ${copied}";
    maintainers = [ "someone" ];
  };
}
