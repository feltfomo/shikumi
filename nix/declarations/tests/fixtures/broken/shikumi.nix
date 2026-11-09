{ types }:
{
  projects = {
    sound = "projects/sound";
    missing = "projects/missing";
    wrong-type = "projects/wrong-type";
    closed-enum = "projects/closed-enum";
    unknown-options = "projects/unknown-options";
    bad-names = "projects/bad-names";
    empty-list = "projects/empty-list";
    conflict = "projects/conflict";
    store-reference = "projects/store-reference";
    location = "projects/location";
    laundered = "projects/laundered";
    dangling = "projects/dangling";
    overlap = "projects/dangling/inner";
    duplicate-a = "projects/duplicate-a";
    duplicate-b = "projects/duplicate-b";
    absent = "projects/absent";
    outside = "../outside";
    extensions = "projects/extensions";
  };
  extensions = {
    team = types.record { owner = types.required types.identifier; };
    meta = types.json;
    layout = types.record { pages = types.required (types.listOf types.glob); };
    broken = "not a schema";
    # Shikumi's own `command` type skips the checks that keep a string out of the document, which
    # is fine for a command line it never copies and wrong for an extension value it does.
    sneaky = {
      _tag = "command";
    };
  };
  owners = { };
}
