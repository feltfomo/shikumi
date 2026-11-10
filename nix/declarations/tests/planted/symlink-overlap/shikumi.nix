# a listed directory that is a symlink to another one: read through it, two projects would share
# one declaration, which the overlap rule, comparing directories as written, can't see.
{
  projects = {
    app = "app";
    mirror = "mirror";
  };
}
