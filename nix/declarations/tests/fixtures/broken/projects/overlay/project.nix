{ shikumi }:
shikumi.project {
  project = {
    summary = "Brings an overlay of its own";
    maintainers = [ "someone" ];
  };
  overrides.hello = {
    reason = "a patched hello";
    release = "upstream takes the patch";
    package = _: prev: prev.hello;
  };
}
