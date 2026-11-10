# a declaration file is called with only the arguments it names.
{ shikumi }:
shikumi.project {
  project = {
    summary = "The documentation site";
    maintainers = [ "writers" ];
    dependsOn = [ "web" ];
  };
  packages = [ "mdbook" ];
  env.DOCS_PORT = "3000";
  extensions.layout.pages = [ "pages/**/*.md" ];
  commands.preview = {
    description = "Preview the docs";
    exec = "preview";
  };
}
