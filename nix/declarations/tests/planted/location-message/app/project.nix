# a read of the store directory the copy lies in, which pure mode refuses with the path in its
# message: on an evaluator that reads the copy where it lies, a path under the cache directory.
{ shikumi }:
shikumi.project {
  project = {
    summary = builtins.toJSON (builtins.readDir (dirOf (dirOf (toString ./.))));
    maintainers = [ "someone" ];
  };
}
