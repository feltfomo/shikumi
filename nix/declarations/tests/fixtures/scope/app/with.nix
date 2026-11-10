# the names a `with` brings in, as nixpkgs' `with pkgs; fetchurl` does: a withheld builtin with no
# global name of its own mustn't take them.
with {
  fetchurl = "from with";
  getFlake = "from with";
  fetchClosure = "from with";
};
[
  fetchurl
  getFlake
  fetchClosure
]
