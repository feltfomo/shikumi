# a repository where most declarations fail to evaluate, each in a different way, beside ones that
# evaluate. every failure comes back as its own project's finding, at the user's line, and the
# others are still checked.
{
  projects = {
    assertion = "projects/assertion";
    call-type = "projects/call-type";
    coerce = "projects/coerce";
    consumer = "projects/consumer";
    library = "projects/library";
    missing-summary = "projects/missing-summary";
    sound = "projects/sound";
    syntax = "projects/syntax";
    thrown = "projects/thrown";
    undefined = "projects/undefined";
    uneven = "projects/uneven";
    utf8 = "projects/utf8";
  };
  catalog = {
    directory = "catalog";
    systems = [ "x86_64-linux" ];
  };
}
