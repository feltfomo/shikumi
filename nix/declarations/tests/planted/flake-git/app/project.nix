# another Git repository, read as a flake at a locked revision: Determinate Nix allows a top-level
# `getFlake` in pure and restricted mode. the harness commits the case's `outside/` there, at a
# fixed date, and writes its location for `@outside@`.
{ shikumi }:
shikumi.project {
  project = {
    summary =
      (builtins.getFlake "git+file://@outside@/flake-git?rev=3bfa9b634b043f6cbf9a86a7b852b4f7f2bae2a0")
      .secret;
    maintainers = [ "someone" ];
  };
}
