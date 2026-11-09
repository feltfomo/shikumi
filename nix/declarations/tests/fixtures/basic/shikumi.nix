{ types }:
{
  projects = {
    core = "libs/core";
    web = "apps/web";
    docs = "docs";
  };
  extensions.layout = types.record { pages = types.required (types.listOf types.glob); };
  extensions.team = types.record {
    owner = types.required types.identifier;
    tier = types.optional (
      types.enum [
        "critical"
        "standard"
      ]
    );
  };
}
