# composition: the manifest, the declarations it lists and the checks across them, folded into the
# `shikumi.registry` document. the document carries every finding as data, and once it carries one
# its projects and remainder are null, so nothing downstream can act on half-checked declarations.
#
# cost grows linearly with the projects composed: one module evaluation per declaration, one pass
# over each declaration's definitions, and attribute-set joins for the cross-record checks. the
# manifest's directories are sorted once to find overlaps. a focused composition evaluates only the
# focused project and what it depends on; every other declaration stays unread.
{ lib, schema }:
let
  inherit (builtins)
    attrNames
    concatMap
    filter
    genericClosure
    isFunction
    length
    mapAttrs
    pathExists
    zipAttrsWith
    ;
  inherit (schema.types)
    path
    attrsOf
    record
    required
    ;
  inherit (schema.internal) withDefault;

  declaration = import ./declaration.nix { inherit lib schema; };

  # the declaration file every listed directory holds.
  declarationName = "project.nix";

  manifestType = record {
    projects = required (attrsOf path);
    extensions = withDefault { } (attrsOf schema.internal.schema);
  };

  components = lib.splitString "/";

  # two listed directories overlap when one is the other or lies inside it. sorted by components,
  # a directory is immediately followed by one inside it whenever any exists, so comparing
  # neighbours finds every directory that contains another. each directory is split once, before
  # the sort, rather than at every comparison.
  overlaps =
    directories:
    let
      sorted = lib.sort (a: b: lib.lists.compareLists lib.compare a.components b.components < 0) (
        map (entry: entry // { components = components entry.directory; }) directories
      );
      pairs = lib.zipLists sorted (lib.drop 1 sorted);
    in
    filter (
      pair:
      pair.fst.directory == pair.snd.directory
      || lib.hasPrefix (pair.fst.directory + "/") pair.snd.directory
    ) pairs;
in
{
  root,
  manifest,
  focus ? null,
}:
let
  ctx = {
    root = toString root;
  };
  manifestFile = toString manifest;

  imported = import manifest;
  manifestValue = if isFunction imported then imported { inherit (schema) types; } else imported;

  manifestChecked = schema.check ctx [ ] manifestType [
    {
      file = manifestFile;
      value = manifestValue;
    }
  ];
  # built once per composition: the repository's extension schemas are the same for every project.
  declarationType = declaration.type (
    lib.filterAttrs (_: type: type != null) (manifestChecked.value.extensions or { })
  );

  directories = lib.filterAttrs (_: directory: directory != null) (
    manifestChecked.value.projects or { }
  );

  overlapFindings = concatMap (
    pair:
    ownedBy pair.snd.name [
      (schema.finding ctx "overlap"
        [
          "projects"
          pair.snd.name
        ]
        [ manifestFile ]
        "lies inside or at `${pair.fst.directory}`, the directory of `${pair.fst.name}`"
      )
    ]
  ) (overlaps (lib.mapAttrsToList (name: directory: { inherit name directory; }) directories));

  # read lazily, so a focused composition reads only the declarations it composes.
  declarations = mapAttrs (
    name: directory:
    let
      file = root + "/${directory}/${declarationName}";
    in
    if pathExists file then
      declaration.check { inherit ctx declarationType file; }
    else
      {
        value = null;
        fields = { };
        findings = [
          (schema.finding ctx "no-declaration"
            [
              "projects"
              name
            ]
            [ manifestFile ]
            "lists `${directory}`, which has no ${declarationName}"
          )
        ];
      }
  ) directories;

  dependenciesOf =
    name:
    filter (dependency: directories ? ${dependency}) (
      declarations.${name}.value.project.dependsOn or [ ]
    );

  focusFindings = lib.optional (focus != null && !(directories ? ${focus})) (
    schema.finding ctx "unknown-reference" [ ] [ manifestFile ]
      "the focused project `${focus}` isn't listed"
  );

  selected =
    if focus == null then
      attrNames directories
    else if directories ? ${focus} then
      map (item: item.key) (genericClosure {
        startSet = [ { key = focus; } ];
        operator = item: map (key: { inherit key; }) (dependenciesOf item.key);
      })
    else
      [ ];

  composed = lib.genAttrs selected (name: declarations.${name});

  fieldOf = result: name: result.fields.${name} or { files = [ ]; };

  # a reference that names nothing, reported against the files that defined the list holding it.
  danglingReferences =
    loc: listResult: known: what:
    map
      (
        missing:
        schema.finding ctx "unknown-reference" loc listResult.files
          "names `${missing}`, which isn't ${what}"
      )
      (
        filter (name: !known name) (
          lib.unique (if listResult.value or null == null then [ ] else listResult.value)
        )
      );

  # the one rule for which project a finding belongs to.
  ownedBy = name: map (finding: finding // { project = name; });

  referenceFindings = concatMap (
    name:
    let
      result = composed.${name};
      project = fieldOf result "project";
      steps = result.value.steps or { };
      stepResults = (fieldOf result "steps").entries or { };
    in
    ownedBy name (
      danglingReferences [ "project" "dependsOn" ] (fieldOf project "dependsOn") (
        dependency: directories ? ${dependency}
      ) "a listed project"
      ++ concatMap (
        step:
        danglingReferences [ "steps" step "after" ] (fieldOf stepResults.${step} "after") (
          other: steps ? ${other}
        ) "a step of `${name}`"
      ) (attrNames stepResults)
    )
  ) selected;

  # commands share the shell's path and services are started by name, so each name has one owner
  # across the composition.
  uniqueNames =
    kind:
    let
      owners = zipAttrsWith (_: values: values) (
        map (
          name:
          mapAttrs (_: entry: {
            project = name;
            inherit (entry) files;
          }) ((fieldOf composed.${name} kind).entries or { })
        ) selected
      );
    in
    concatMap (
      entry:
      let
        claims = owners.${entry};
      in
      lib.optional (length claims > 1) (
        schema.finding ctx "duplicate" [ kind entry ] (concatMap (claim: claim.files) claims)
          "is declared by more than one project: ${
            lib.concatMapStringsSep ", " (claim: "`${claim.project}`") claims
          }"
      )
    ) (attrNames owners);

  diagnostics =
    manifestChecked.findings
    ++ overlapFindings
    ++ focusFindings
    ++ concatMap (name: ownedBy name composed.${name}.findings) selected
    ++ referenceFindings
    ++ uniqueNames "commands"
    ++ uniqueNames "services";

  accepted = diagnostics == [ ];

  render = name: result: {
    directory = directories.${name};
    inherit (result.value.project) summary maintainers dependsOn;
    steps = mapAttrs (_: step: { inherit (step) description after; }) result.value.steps;
    commands = mapAttrs (_: entry: { inherit (entry) description; }) result.value.commands;
    services = mapAttrs (_: service: { inherit (service) description restart; }) result.value.services;
    inherit (result.value) extensions;
  };
in
{
  document = {
    schema = "shikumi.registry";
    version = {
      major = 1;
      minor = 0;
    };
    focus = if focusFindings == [ ] then focus else null;
    projects = if accepted then mapAttrs render composed else null;
    remainder =
      if accepted && focus == null then
        {
          # the remainder is every source outside these directories.
          excludes = lib.sort (a: b: a < b) (lib.attrValues directories);
        }
      else
        null;
    inherit diagnostics;
  };
}
