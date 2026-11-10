# a manifest that says which arguments it was offered.
arguments: {
  projects.app = "app";
  catalog = {
    directory = "catalog";
    systems = [ "x86_64-linux" ];
  };
  offered = builtins.attrNames arguments;
}
