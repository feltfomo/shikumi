# a manifest that says which arguments it was offered.
arguments: {
  projects.app = "app";
  offered = builtins.attrNames arguments;
}
