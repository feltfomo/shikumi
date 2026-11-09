# the catalog: the one place a repository's packages come from. its file is called with `lib` and
# names every package a shell or step may use under `packages`, each by its attribute path in the
# pinned nixpkgs; `overrides` is the one overlay, an entry per nixpkgs attribute, each with why it
# exists and what lets it go. the catalog never sees a package set, so a package that differs from
# nixpkgs' can only come from an override: there's no `pkgs.extend` or `.override` to reach for,
# and an entry means the same attribute on every system.
#
# what comes back for the checker is, for one system, the catalog's value serialized like any
# declaration, which package each catalog name resolves to in that system's package set with the
# overlay applied, and what every package reachable from them is called and propagates. a package
# is known by its `name`, which nixpkgs makes from its `pname` and `version`, and the checker
# compares the `pname` and `meta.mainProgram` it's given: that's what evaluation can see of what a
# package puts on the path. nothing here forces a package's `outPath` or `drvPath`, so no store path
# is computed and nothing is built or read from a build; the toolchain context, which does resolve
# store paths, reads the same packages from here.
#
# the overlay is built from the entries the catalog declares, which can't see the package set they
# change. an entry whose `package` isn't a function is left out of it, and the checker reports the
# entry; one that is gets `final` and `prev` as an overlay function does.
#
# cost: one evaluation of the pinned nixpkgs per system per run, whatever the number of projects,
# and one visit to each package reachable from the catalog through propagation.
{
  lib,
  forced,
  serialize,
  call,
  # the nixpkgs source tree, imported for each system it's asked for.
  nixpkgs,
  # what the catalog file is called with.
  arguments,
}:
let
  inherit (builtins)
    concatMap
    filter
    genericClosure
    isAttrs
    isFunction
    isList
    isString
    listToAttrs
    mapAttrs
    parseDrvName
    unsafeDiscardStringContext
    ;

  # every attribute through which a package carries another into whatever depends on it. which of
  # them reach a shell's path depends on how the shell takes the package: as a build input, on a
  # native build, every one of the six does; as a `mkShell` package, only `propagatedBuildInputs`,
  # `depsHostHostPropagated` and `depsTargetTargetPropagated` do; and `propagatedUserEnvPkgs`
  # reaches a user environment built from it. the rule reads all of them, so a conflict is found
  # however the shell takes its packages.
  propagation = [
    "propagatedBuildInputs"
    "propagatedNativeBuildInputs"
    "depsBuildBuildPropagated"
    "depsBuildTargetPropagated"
    "depsHostHostPropagated"
    "depsTargetTargetPropagated"
    "propagatedUserEnvPkgs"
  ];

  # a value nixpkgs builds: a derivation with a name. a derivation given a `pname` and no
  # `version` has none, and reading it would fail. `forced` frames the forcing, since every package
  # reached here is one the catalog chose or one a chosen package carries.
  isPackage =
    value:
    isAttrs (forced value)
    && forced (value.type or null) == "derivation"
    && isString (forced (value.name or null));

  nameOf = package: unsafeDiscardStringContext package.name;

  # the packages a package propagates, in the order its attributes list them. nixpkgs keeps a nested
  # list, which setup reads as its elements, and an element that's null, text or a path isn't a
  # package anything reads here.
  propagated =
    package:
    filter isPackage (
      concatMap (
        attribute:
        let
          value = forced (package.${attribute} or [ ]);
        in
        if isList value then lib.flatten value else [ ]
      ) propagation
    );

  text = value: if isString (forced value) then unsafeDiscardStringContext value else null;

  describe =
    package:
    let
      parsed = parseDrvName (nameOf package);
      pname = text (package.pname or null);
      version = text (package.version or null);
      main = text ((forced (package.meta or { })).mainProgram or null);
    in
    {
      pname = if pname == null || pname == "" then parsed.name else pname;
      version =
        let
          written = if version == null then parsed.version else version;
        in
        if written == "" then null else written;
      mainProgram = if main == "" then null else main;
      propagates = map nameOf (propagated package);
    };
in
root:
{ file, system }:
let
  serialized = serialize (toString root);
  known = builtins.elem system lib.systems.doubles.all;

  declared = call arguments (import (root + "/${file}"));
  field = name: if isAttrs declared then forced (declared.${name} or null) else null;
  attrsIn = value: if isAttrs value then value else { };

  overrides = lib.filterAttrs (
    _: entry: isAttrs (forced entry) && isFunction (forced (entry.package or null))
  ) (attrsIn (field "overrides"));

  pkgs = import nixpkgs {
    localSystem = { inherit system; };
    # given, so nixpkgs reads no configuration or overlay of the host's.
    config = { };
    overlays = [ (final: prev: mapAttrs (_: entry: entry.package final prev) overrides) ];
  };

  # each entry's package, or nothing where its path names no package on this system. an alias
  # nixpkgs has removed throws, which counts as naming none; the checker reports the entry.
  resolve =
    path:
    if isString (forced path) && !builtins.hasContext path then
      let
        found = builtins.tryEval (
          let
            value = lib.attrByPath (lib.splitString "." path) null pkgs;
          in
          if isPackage value then value else null
        );
      in
      if found.success then found.value else null
    else
      null;
  packages = lib.filterAttrs (_: package: package != null) (
    mapAttrs (_: resolve) (attrsIn (field "packages"))
  );

  reachable = genericClosure {
    startSet = lib.mapAttrsToList (_: package: {
      key = nameOf package;
      inherit package;
    }) packages;
    operator =
      { package, ... }:
      map (package: {
        key = nameOf package;
        inherit package;
      }) (propagated package);
  };
in
# a system nixpkgs doesn't know has no package set to read.
if !known then
  null
else
  {
    value = serialized.value declared;
    packages = mapAttrs (_: nameOf) packages;
    graph = listToAttrs (
      map (item: {
        name = item.key;
        value = describe item.package;
      }) reachable
    );
  }
