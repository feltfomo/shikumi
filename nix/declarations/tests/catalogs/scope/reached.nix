# the build `callPackage` makes: hello, named for what this file sees.
{ hello }:
hello.overrideAttrs {
  pname =
    if builtins ? getFlake || builtins ? fetchTree || builtins.builtins ? getFlake then
      "withheld-seen"
    else
      "withheld-unseen";
}
