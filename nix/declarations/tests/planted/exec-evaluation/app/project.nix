# `builtins.exec` exists only where configuration turns it on, and it runs anything, a build
# included. the check's control turns it on through `NIX_CONFIG`.
{
  project = {
    summary = builtins.exec [
      "/bin/sh"
      "-c"
      "echo '\"Ran a program while evaluating\"'"
    ];
    maintainers = [ "someone" ];
  };
}
