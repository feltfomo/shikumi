# fails inside nixpkgs' `lib`, which the evaluator reaches only once Shikumi reads the list, and with
# no position in this file.
{ shikumi, lib, ... }:
shikumi.project {
  project = {
    summary = "Maintainers by number";
    maintainers = map lib.toInt [ "someone" ];
  };
}
