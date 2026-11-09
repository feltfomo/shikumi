# composition: reads the manifest, names each listed project's declaration file, and serializes the
# manifest and each project's declaration for the checker. nothing here decides whether a
# declaration is right; the checker, in Rust, owns that, and every value reaches it as it was
# written. what this file does decide is which files to read, so it guards only that: a manifest
# entry that doesn't name exactly one file inside the repository is read as having none.
#
# each project's declaration is a value of its own, so whoever evaluates them can evaluate each one
# apart and keep a failure to the project that caused it. every value a user wrote is first forced
# through `forced`, so a failure in it reads as the user's. which projects a composition holds is
# the caller's to choose, from `files` and what each declaration's `dependsOn` serializes to.
#
# cost grows linearly with the projects composed: one import and one serialization per declaration.
# `read` takes a declaration file `files` named and reads the manifest no more, so evaluating some
# projects apart costs the same however many the manifest lists.
{
  lib,
  serialize,
  isProject,
  forced,
  # what a project's declaration file is called with, and what the manifest is.
  arguments,
  manifestArguments,
  # the file names the layer shares with the registry's Rust.
  layer,
}:
let
  inherit (builtins)
    functionArgs
    hasContext
    intersectAttrs
    isAttrs
    isFunction
    isString
    pathExists
    seq
    ;

  # the declaration file every listed directory holds.
  declarationName = layer.declaration.name;

  # calls a file's function with the arguments it names, or with all of them when it names none.
  # a function that requires an argument nobody gives it stays a function, which the checker
  # reports, rather than failing evaluation for every other declaration too.
  call =
    available: imported:
    let
      value = forced imported;
      named = functionArgs value;
      unmet = lib.filterAttrs (name: optional: !optional && !(available ? ${name})) named;
    in
    if !isFunction value || unmet != { } then
      value
    else
      forced (value (if named == { } then available else intersectAttrs named available));
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

  # the file a directory names. path addition normalizes `.`, `..`, doubled and trailing
  # slashes, so a directory written any other way than plainly names a different file than it
  # says, and none is read; the checker reports the entry itself. a directory where the file
  # belongs isn't read either, since importing it would read its `default.nix`.
  fileOf =
    directory:
    if isString (forced directory) && !hasContext directory then
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
  files = builtins.mapAttrs (
    _: directory:
    let
      file = fileOf directory;
    in
    if file == null then null else serialized.relative (toString file)
  ) listed;
}
