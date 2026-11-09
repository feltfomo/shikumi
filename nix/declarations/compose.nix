# composition: reads the manifest, imports the declaration of every project the composition holds,
# and serializes both into the `shikumi.declarations` document the checker reads. nothing here
# decides whether a declaration is right; the checker, in Rust, owns that, and every value reaches it
# as it was written. what this file does decide is which files to read, so it guards only that: a
# manifest entry that doesn't name exactly one file inside the repository is read as having none.
#
# cost grows linearly with the projects composed: one import and one serialization per declaration.
# a focused composition imports only the focused project and the projects its `dependsOn` names,
# transitively; every other declaration stays unread.
{
  lib,
  serialize,
  isProject,
  # what a project's declaration file is called with.
  arguments,
  types,
}:
let
  inherit (builtins)
    filter
    functionArgs
    genericClosure
    hasContext
    intersectAttrs
    isAttrs
    isFunction
    isList
    isString
    pathExists
    ;

  # the declaration file every listed directory holds.
  declarationName = "project.nix";

  # calls a file's function with the arguments it names, or with all of them when it names none.
  # a function that requires an argument nobody gives it stays a function, which the checker
  # reports, rather than failing evaluation for every other declaration too.
  call =
    available: value:
    let
      named = functionArgs value;
      unmet = lib.filterAttrs (name: optional: !optional && !(available ? ${name})) named;
    in
    if !isFunction value || unmet != { } then
      value
    else
      value (if named == { } then available else intersectAttrs named available);
in
{
  root,
  manifest,
  focus ? null,
}:
let
  rootString = toString root;
  serialized = serialize rootString;

  manifestValue = call { inherit types lib; } (import manifest);

  listed =
    if isAttrs manifestValue && isAttrs (manifestValue.projects or null) then
      manifestValue.projects
    else
      { };

  # the file a directory names. path addition normalizes `.`, `..`, doubled and trailing slashes,
  # so a directory written any other way than plainly names a different file than it says, and none
  # is read; the checker reports the entry itself. a directory where the file belongs isn't read
  # either, since importing it would read its `default.nix`.
  fileOf =
    directory:
    if isString directory && !hasContext directory then
      let
        file = root + "/${directory}/${declarationName}";
      in
      if
        toString file == "${rootString}/${directory}/${declarationName}"
        && pathExists file
        && builtins.readFileType file != "directory"
      then
        file
      else
        null
    else
      null;

  # read lazily, so a focused composition reads only the declarations it composes.
  declarations = builtins.mapAttrs (
    _: directory:
    let
      file = fileOf directory;
    in
    if file == null then null else call arguments (import file)
  ) listed;

  # which projects a declaration depends on, for selecting a focused composition only. a value that
  # isn't a list of listed names selects nothing more; the checker reports it. `or` also answers a
  # selection through something that isn't an attribute set, on every implementation.
  dependenciesOf =
    name:
    let
      declaration = declarations.${name};
      dependsOn = declaration.declaration.project.dependsOn or [ ];
    in
    if isProject declaration && isList dependsOn then
      filter (dependency: isString dependency && listed ? ${dependency}) dependsOn
    else
      [ ];

  selected =
    if focus == null then
      builtins.attrNames listed
    else if listed ? ${focus} then
      map (item: item.key) (genericClosure {
        startSet = [ { key = focus; } ];
        operator = item: map (key: { inherit key; }) (dependenciesOf item.key);
      })
    else
      [ ];
in
{
  document = {
    schema = "shikumi.declarations";
    version = {
      major = 1;
      minor = 0;
    };
    inherit focus;
    manifest = {
      file = serialized.relative (toString manifest);
      value = serialized.value manifestValue;
    };
    # null where the manifest's entry names no declaration file.
    projects = lib.genAttrs selected (
      name:
      let
        declaration = declarations.${name};
        factory = isProject declaration;
      in
      if declaration == null then
        null
      else
        {
          inherit factory;
          value = serialized.value (if factory then declaration.declaration else declaration);
        }
    );
  };
}
