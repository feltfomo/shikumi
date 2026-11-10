# an override whose name is built from where the evaluator reads the copy, so the toolchain context
# would carry a store path that differs with the cache directory.
_: {
  packages.located = "hello";
  overrides.hello = {
    reason = "names the package from the copy's location";
    release = "the planted case goes";
    package =
      _: prev:
      prev.hello.overrideAttrs {
        pname = "hello-${toString (builtins.stringLength (toString ./.))}";
      };
  };
}
