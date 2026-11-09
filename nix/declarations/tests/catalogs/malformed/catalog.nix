# overlay entries the checker refuses: none of them can stop the package set from evaluating.
_: {
  packages = {
    hello = "hello";
    # as an alias nixpkgs has removed does, the package throws when it's read.
    gone = "gone";
  };
  overrides = {
    hello = {
      reason = "its build isn't a function";
      release = "never";
      package = "not a function";
    };
    cowsay = "not an entry";
    figlet.reason = "no build at all";
    gone = {
      reason = "stands for a removed alias";
      release = "never";
      package = _: _: throw "gone was removed";
    };
  };
}
