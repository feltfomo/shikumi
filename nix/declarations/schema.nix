# the field types declarations, the manifest and repository extension schemas are written in, and
# the stage that checks definitions against them. checking never throws on user input: every problem
# becomes a finding that names its option path and the files that defined it, so one pass over a
# repository reports all of them. the module system still discharges `mkIf`, `mkMerge` and
# priorities first, through `lib.modules.mergeDefinitions`, so a definition means what it would mean
# in any module.
{ lib }:
let
  inherit (builtins)
    all
    attrNames
    concatMap
    elem
    filter
    hasContext
    head
    isAttrs
    isBool
    isFloat
    isInt
    isList
    isString
    length
    mapAttrs
    match
    zipAttrsWith
    ;

  scalar = tag: { _tag = tag; };

  types = {
    # non-blank text the document may carry.
    text = scalar "text";
    # a lowercase name: a letter, then letters, digits and hyphens.
    identifier = scalar "identifier";
    # a normalized path relative to the repository root.
    path = scalar "path";
    # a pattern over paths relative to the repository root, such as `docs/**/*.md`. Shikumi checks
    # that it stays inside the repository and leaves its meaning to whoever reads it.
    glob = scalar "glob";
    enum = values: {
      _tag = "enum";
      inherit values;
    };
    # any JSON value: strings, numbers, booleans, null, lists and attribute sets of them.
    json = scalar "json";
    listOf = element: {
      _tag = "list";
      inherit element;
      nonEmpty = false;
    };
    nonEmptyListOf = element: {
      _tag = "list";
      inherit element;
      nonEmpty = true;
    };
    # an attribute set keyed by identifiers.
    attrsOf = element: {
      _tag = "attrs";
      inherit element;
    };
    record = fields: {
      _tag = "record";
      inherit fields;
    };
    required = type: {
      inherit type;
      required = true;
    };
    # absent stays absent: the field is left out rather than given a value nobody wrote.
    optional = type: {
      inherit type;
      required = false;
    };
  };

  # the parts of the vocabulary only Shikumi's own records use. a default is part of the schema
  # Shikumi publishes, never a value invented for a required field, and repository extensions don't
  # get one because Shikumi passes their values through exactly as written.
  internal = {
    # a command line. it never reaches the document, so unlike text it may name store paths and
    # the checkout's location.
    command = scalar "command";
    # a schema a repository supplies for its extension namespace, built from `types`.
    schema = scalar "schema";
    withDefault = default: type: {
      inherit type default;
      required = false;
    };
  };

  problem = code: message: { inherit code message; };

  # a store path's name: the store directory, then a hash in Nix's base-32 alphabet. matching the
  # text catches a path whose context was stripped, by `match` or `substring` as much as by
  # `unsafeDiscardStringContext`.
  storePathPattern = ".*${lib.escapeRegex builtins.storeDir}/[0-9a-df-np-sv-z]{32}-.*";

  # what keeps a string out of the document: a store reference would make the document depend on a
  # store path, and the repository's location differs between worktrees of one commit. the location
  # comes before the pattern: a checkout that is itself a store path, as a fixture in a build is,
  # would otherwise report its own location as a store reference.
  stringProblems =
    ctx: value:
    if hasContext value then
      [ storeReference ]
    else if lib.hasInfix ctx.root value then
      [ (problem "location" "contains the repository's location, which differs between worktrees") ]
    else if match storePathPattern value != null then
      [ storeReference ]
    else
      [ ];

  storeReference = problem "store-reference" "refers to a store path, and the registry document carries none";

  notString = [ (problem "type" "must be a string") ];

  blank = value: match "[[:space:]]*" value != null;

  validPath =
    value:
    match "[^/]+(/[^/]+)*" value != null
    && all (part: part != "." && part != "..") (lib.splitString "/" value);

  jsonProblems =
    ctx: value:
    if isString value then
      stringProblems ctx value
    else if isBool value || isInt value || isFloat value || value == null then
      [ ]
    else if isList value then
      concatMap (jsonProblems ctx) value
    # a derivation or anything else with `outPath` would serialize as a store path.
    else if isAttrs value && !(value ? outPath) then
      concatMap (name: stringProblems ctx name ++ jsonProblems ctx value.${name}) (attrNames value)
    else
      [
        (problem "type" "must be JSON data: a string, number, boolean, null, list or attribute set of them")
      ];

  textProblems =
    ctx: value:
    if !isString value then
      notString
    else if blank value then
      [ (problem "invalid" "must not be empty or blank") ]
    else
      stringProblems ctx value;

  identifierProblems =
    ctx: value:
    if !isString value then
      notString
    else if match "[a-z][a-z0-9-]*" value == null then
      [
        (problem "invalid" "must be an identifier: a lowercase letter, then lowercase letters, digits and hyphens")
      ]
    else
      stringProblems ctx value;

  scalarKind = problems: {
    check = checkScalar;
    inherit problems;
    shape = _: true;
    public = true;
  };

  # every kind of type, once: how to check a value of it, whether its own shape is sound, and
  # whether a repository's extension schema may use it. `command` and `schema` are Shikumi's own:
  # a command skips the checks that keep a string out of the document, which only holds for a value
  # the document never carries, and an extension value always reaches the document.
  kinds = {
    text = scalarKind (ctx: _: textProblems ctx);
    identifier = scalarKind (ctx: _: identifierProblems ctx);
    path = scalarKind (
      ctx: _: value:
      if !isString value then
        notString
      else if !validPath value then
        [
          (problem "invalid" "must be a normalized relative path: no leading or trailing slash, no empty, `.` or `..` part")
        ]
      else
        stringProblems ctx value
    );
    glob = scalarKind (
      ctx: _: value:
      if !isString value then
        notString
      else if blank value || lib.hasPrefix "/" value || elem ".." (lib.splitString "/" value) then
        [ (problem "invalid" "must be a pattern relative to the repository root, with no `..` part") ]
      else
        stringProblems ctx value
    );
    enum =
      scalarKind (
        ctx: type: value:
        if !isString value then
          notString
        else if !elem value type.values then
          [ (problem "enum" "must be one of ${lib.concatMapStringsSep ", " (v: "`${v}`") type.values}") ]
        else
          stringProblems ctx value
      )
      // {
        shape = type: isList type.values && all isString type.values;
      };
    json = scalarKind (ctx: _: jsonProblems ctx);
    command =
      scalarKind (
        _: _: value:
        if !isString value then
          notString
        else if blank value then
          [ (problem "invalid" "must not be empty or blank") ]
        else
          [ ]
      )
      // {
        public = false;
      };
    schema =
      scalarKind (
        _: _: value:
        if validType value then
          [ ]
        else
          [ (problem "invalid" "must be a schema built from `shikumi.lib.types`") ]
      )
      // {
        public = false;
      };
    list = {
      check = checkList;
      shape = type: validType type.element;
      public = true;
    };
    attrs = {
      check = checkAttrs;
      shape = type: validType type.element;
      public = true;
    };
    record = {
      check = checkRecord;
      shape =
        type:
        isAttrs type.fields
        && all (
          name:
          let
            field = type.fields.${name};
          in
          isAttrs field && isBool (field.required or null) && validType (field.type or null)
        ) (attrNames type.fields);
      public = true;
    };
  };

  # a repository's extension schema has to be built from the public vocabulary, all the way down.
  validType =
    type:
    let
      kind = kinds.${type._tag or ""} or null;
    in
    isAttrs type && kind != null && kind.public && kind.shape type;

  # a file the document may name: relative to the repository root, or null when it lies outside.
  relative =
    ctx: file:
    let
      prefix = ctx.root + "/";
    in
    if lib.hasPrefix prefix file then lib.removePrefix prefix file else null;

  finding =
    ctx: code: loc: files: message:
    let
      named = map (relative ctx) files;
      inside = lib.unique (filter (file: file != null) named);
    in
    {
      severity = "error";
      inherit code;
      # the composition names the project a finding belongs to, where it belongs to one.
      project = null;
      option = if loc == [ ] then null else lib.showOption loc;
      files = inside;
      message =
        message
        + lib.optionalString (length inside != length named) " (defined in a file outside the repository)";
    };

  filesOf = map (def: def.file);

  final = loc: defs: (lib.modules.mergeDefinitions loc lib.types.raw defs).defsFinal;

  # each check returns whether anything was defined, the value (null where it's invalid), the
  # findings, the defining files, and for a record the results of its fields, which cross-record
  # checks read to name the file a reference came from.
  absent = {
    defined = false;
    value = null;
    findings = [ ];
    files = [ ];
  };

  checkScalar =
    ctx: loc: type: defs:
    let
      defined = final loc defs;
      first = head defined;
      problems = kinds.${type._tag}.problems ctx type first.value;
    in
    if defined == [ ] then
      absent
    else if length defined > 1 && !all (def: def.value == first.value) defined then
      {
        defined = true;
        value = null;
        findings = [
          (finding ctx "conflict" loc (filesOf defined) "is defined more than once with different values")
        ];
        files = filesOf defined;
      }
    else
      {
        defined = true;
        value = if problems == [ ] then first.value else null;
        findings = map (p: finding ctx p.code loc [ first.file ] p.message) problems;
        files = filesOf defined;
      };

  checkList =
    ctx: loc: type: defs:
    let
      defined = final loc defs;
      perDefinition = map (
        def:
        if !isList def.value then
          {
            items = [ ];
            findings = [ (finding ctx "type" loc [ def.file ] "must be a list") ];
          }
        else
          let
            results = map (
              item:
              check ctx loc type.element [
                {
                  inherit (def) file;
                  value = item;
                }
              ]
            ) def.value;
          in
          {
            items = map (result: result.value) results;
            findings = concatMap (result: result.findings) results;
          }
      ) defined;
      findings = concatMap (part: part.findings) perDefinition;
      items = concatMap (part: part.items) perDefinition;
      empty =
        if type.nonEmpty && findings == [ ] && items == [ ] then
          [ (finding ctx "invalid" loc (filesOf defined) "must not be empty") ]
        else
          [ ];
    in
    if defined == [ ] then
      absent
    else
      {
        defined = true;
        value = if findings == [ ] then items else null;
        findings = findings ++ empty;
        files = filesOf defined;
      };

  # splits attribute-set definitions by key, keeping each value's file, in one linear pass.
  byKey =
    loc: defined:
    let
      sets = filter (def: isAttrs def.value) defined;
      notSets = filter (def: !isAttrs def.value) defined;
    in
    {
      inherit notSets;
      keys = zipAttrsWith (_: values: values) (
        map (
          def:
          mapAttrs (_: value: {
            inherit (def) file;
            inherit value;
          }) def.value
        ) sets
      );
      typeFindings =
        ctx: map (def: finding ctx "type" loc [ def.file ] "must be an attribute set") notSets;
    };

  checkAttrs =
    ctx: loc: type: defs:
    let
      defined = final loc defs;
      split = byKey loc defined;
      results = mapAttrs (
        name: entries:
        let
          key = identifierProblems ctx name;
          result = check ctx (loc ++ [ name ]) type.element entries;
        in
        result
        // {
          findings =
            map (p: finding ctx p.code (loc ++ [ name ]) (filesOf entries) "name ${p.message}") key
            ++ result.findings;
        }
      ) split.keys;
      findings = split.typeFindings ctx ++ concatMap (name: results.${name}.findings) (attrNames results);
    in
    if defined == [ ] then
      absent
    else
      {
        defined = true;
        value = mapAttrs (_: result: result.value) (lib.filterAttrs (_: result: result.defined) results);
        inherit findings;
        files = filesOf defined;
        entries = results;
      };

  checkRecord =
    ctx: loc: type: defs:
    let
      defined = final loc defs;
      split = byKey loc defined;
      unknown = filter (name: !(type.fields ? ${name})) (attrNames split.keys);
      fields = mapAttrs (
        name: field:
        let
          result =
            if split.keys ? ${name} then check ctx (loc ++ [ name ]) field.type split.keys.${name} else absent;
        in
        if result.defined then
          result
        else if field.required then
          absent
          // {
            findings = [
              (finding ctx "missing" (loc ++ [ name ]) (filesOf defined) "is required but not defined")
            ];
          }
        else if field ? default then
          absent
          // {
            defined = true;
            value = field.default;
          }
        else
          absent
      ) type.fields;
      findings =
        split.typeFindings ctx
        ++ map (
          name:
          finding ctx "unknown-option" (loc ++ [ name ]) (filesOf split.keys.${name}) "is not an option here"
        ) unknown
        ++ concatMap (name: fields.${name}.findings) (attrNames fields);
    in
    if defined == [ ] then
      absent
    else
      {
        defined = true;
        value = mapAttrs (_: result: result.value) (lib.filterAttrs (_: result: result.defined) fields);
        inherit findings fields;
        files = filesOf defined;
      };

  check =
    ctx: loc: type: defs:
    kinds.${type._tag}.check ctx loc type defs;
in
{
  inherit
    types
    internal
    validType
    check
    finding
    ;
}
