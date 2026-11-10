# a directory outside the repository, read as a flake by a locked `path:` reference: Determinate
# Nix allows a top-level `getFlake` in pure and restricted mode. the harness puts the case's
# `outside/` there and writes its location for `@outside@`.
{ shikumi }:
shikumi.project {
  project = {
    summary =
      (builtins.getFlake "path:@outside@/flake-path?narHash=sha256-cJcb1iEopQDF47/EO243eokXjYMyHY3f+7PcxpUvVXE=")
      .secret;
    maintainers = [ "someone" ];
  };
}
