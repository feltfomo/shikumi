# one project and a catalog whose override is named from where the evaluator reads the copy.
_: {
  projects.app = "app";
  catalog = {
    directory = "catalog";
    systems = [ "x86_64-linux" ];
  };
}
