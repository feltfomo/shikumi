# composition: reads the manifest, names each listed project's declaration file and the catalog's,
# and serializes the manifest and each project's declaration for the checker. nothing here decides
# whether a declaration is right; the checker, in Rust, owns that, and every value reaches it as it
# was written. what this file does decide is which files to read, so it guards only that: a
# manifest entry that doesn't name exactly one file inside the repository is read as having none.
#
# each project's declaration is a value of its own, so whoever evaluates them can evaluate each one
# apart and keep a failure to the project that caused it. every value a user wrote is first forced
# through `forced`, so a failure in it reads as the user's. which projects a composition holds is
# the caller's to choose, from `files` and the projects each declaration's `dependsOn` and
# toolchain imports serialize to. the catalog is read apart from all of them, by `catalog.nix`.
#
# cost grows linearly with the projects composed: one import and one serialization per declaration.
# `read` takes a declaration file `files` named and reads the manifest no more, so evaluating some
# projects apart costs the same however many the manifest lists.
{
  serialize,
  isProject,
  forced,
  call,
  # what a project's declaration file is called with, and what the manifest is.
  arguments,
  manifestArguments,
  # the file names the layer shares with the registry's Rust.
  layer,
}:
let
  inherit (builtins)
    hasContext
    isAttrs
    isList
    isString
    pathExists
    seq
    ;

  # the declaration file every listed directory holds, and the one the catalog's directory holds.
  declarationName = layer.declaration.name;
  catalogName = layer.catalog.name;
in
{
  root,
  manifest,
}:
let
  rootString = toString root;
  serialized = serialize rootString;

  manifestValue = call manifestArguments (import manifest);

  listed =
    if isAttrs manifestValue && isAttrs (forced (manifestValue.projects or null)) then
      manifestValue.projects
    else
      { };

  # the file named `name` in a directory the manifest gives. path addition normalizes `.`, `..`,
  # doubled and trailing slashes, so a directory written any other way than plainly names a
  # different file than it says, and none is read; the checker reports the entry itself. a
  # directory where the file belongs isn't read either, since importing it would read its
  # `default.nix`. the file comes back relative to the root, or null.
  fileIn =
    name: directory:
    if isString (forced directory) && !hasContext directory then
      let
        file = root + "/${directory}/${name}";
      in
      if
        toString file == "${rootString}/${directory}/${name}"
        && pathExists file
        && builtins.readFileType file != "directory"
      then
        serialized.relative (toString file)
      else
        null
    else
      null;

  declaredCatalog = if isAttrs manifestValue then forced (manifestValue.catalog or null) else null;

  # one project's serialized declaration, read from its file.
  read =
    file:
    let
      declaration = call arguments (import (root + "/${file}"));
      # the mark is the user's value; reading it is the factory's.
      factory = seq (forced (declaration._type or null)) (isProject declaration);
    in
    {
      inherit factory;
      value = serialized.value (if factory then declaration.declaration else declaration);
    };
in
{
  inherit read;

  manifest = {
    file = serialized.relative (toString manifest);
    value = serialized.value manifestValue;
  };

  # each listed project's declaration file, relative to the root, or null where its entry names
  # none.
  files = builtins.mapAttrs (_: fileIn declarationName) listed;

  # the catalog the manifest declares: its file, or null where its directory holds none, and the
  # systems its package set is read for, as far as they're text. null when the manifest declares
  # no catalog. what's wrong with the declaration is the checker's to report from the manifest.
  catalog =
    if isAttrs declaredCatalog then
      let
        systems = forced (declaredCatalog.systems or null);
      in
      {
        file = fileIn catalogName (declaredCatalog.directory or null);
        systems =
          if isList systems then
            builtins.filter (system: isString (forced system) && !hasContext system) systems
          else
            [ ];
      }
    else
      null;
}
