{
  project = {
    summary = "The documentation site";
    maintainers = [ "writers" ];
    dependsOn = [ "web" ];
  };
  extensions.layout.pages = [ "pages/**/*.md" ];
  commands.preview = {
    description = "Preview the docs";
    exec = "preview";
  };
}
