# a catalog that fails to evaluate is the catalog's finding alone: the projects that pick from it
# are still checked, and nothing is checked against it.
_: {
  packages.jdk = throw "the catalog's JDK isn't chosen yet";
}
