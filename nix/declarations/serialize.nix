# turns any value a declaration or manifest holds into a tagged tree the checker reads, with the
# source position of every attribute. it never throws on a value of the wrong kind: a function, a
# path or a derivation becomes a node that says what it was, so Rust can report it. the only things
# that stop it are those Nix itself can't get past, such as a `throw` inside the value.
#
# positions come from `builtins.unsafeGetAttrPos`, which CppNix, Determinate Nix and Lix answer
# alike: the start of the attribute path that defined the attribute, in the file that wrote it, so
# an attribute brought in by `import` and `//` keeps the position in its own file. a position is
# null, and the checker falls back to the nearest enclosing field, where the evaluator has none to
# give: list elements, attributes `builtins.mapAttrs` creates, and attributes whose position lies
# outside the repository, such as the ones nixpkgs' `genAttrs` makes, which point into nixpkgs.
# files are relative to the repository root, so the result doesn't depend on where the checkout is.
#
# cost is linear in the size of the value: one visit per value and one position lookup per
# attribute, plus one count of each top-level field, stopped at `budget`. a value that refers to
# itself or shares one value many times, such as a package set, is finite to Nix but unbounded or
# exponential to walk, so a top-level field that holds more than `budget` values, or nests deeper
# than `maxDepth`, becomes one `too-large` node instead of being walked, carrying both limits so
# the checker's finding states them from here.
{ lib }:
let
  inherit (builtins)
    attrValues
    foldl'
    hasContext
    isAttrs
    isList
    mapAttrs
    typeOf
    unsafeDiscardStringContext
    unsafeGetAttrPos
    ;

  maxDepth = 32;
  budget = 100000;

  # a derivation, or anything else with an `outPath`, stands for a store path. it isn't entered:
  # its attributes are the package's, not the declaration's.
  isOpaque = value: value ? outPath || value.type or null == "derivation";

  # how many values `value` holds, counting itself, or more than `limit` once there are more. the
  # count stops as soon as it passes the limit, so it costs at most `limit` visits whatever the
  # value's size, and nesting past `maxDepth` counts as past the limit.
  size =
    depth: limit: value:
    let
      over = limit + 1;
      children =
        if isList value then
          value
        else if isAttrs value && !isOpaque value then
          attrValues value
        else
          [ ];
    in
    if children == [ ] then
      1
    else if depth >= maxDepth then
      over
    else
      foldl' (
        count: child: if count > limit then count else count + size (depth + 1) (limit - count) child
      ) 1 children;
in
root:
let
  prefix = "${root}/";

  # a file inside the repository, relative to its root, or null for one outside it.
  relative =
    file:
    if file == root then
      "."
    else if lib.hasPrefix prefix file then
      lib.removePrefix prefix file
    else
      null;

  position =
    set: name:
    let
      found = unsafeGetAttrPos name set;
      file = if found == null then null else relative found.file;
    in
    if file == null then
      null
    else
      {
        inherit file;
        inherit (found) line column;
      };

  # one entry per kind `builtins.typeOf` names, so each value costs one lookup, and a kind with no
  # entry fails here by name rather than being read as something else.
  kinds = {
    string = value: {
      kind = "string";
      # the text goes on and the context becomes a flag, so the result refers to no store path
      # and printing it builds nothing.
      value = unsafeDiscardStringContext value;
      context = hasContext value;
    };
    path = value: {
      kind = "path";
      value = relative (toString value);
    };
    bool = value: {
      kind = "bool";
      inherit value;
    };
    int = value: {
      kind = "int";
      inherit value;
    };
    # infinity and NaN have no JSON form, and only a finite float minus itself is zero.
    float =
      value:
      if value - value == 0.0 then
        {
          kind = "float";
          inherit value;
        }
      else
        { kind = "non-finite"; };
    null = _: { kind = "null"; };
    list = value: {
      kind = "list";
      items = map node value;
    };
    lambda = _: { kind = "function"; };
    set = value: if isOpaque value then { kind = "derivation"; } else fields node value;
  };

  node =
    value:
    let
      kind = typeOf value;
    in
    (kinds.${kind} or (throw "shikumi: serialization has no case for a Nix value of kind ${kind}"))
      value;

  fields = serialize: value: {
    kind = "attrs";
    fields = mapAttrs (name: field: {
      at = position value name;
      value = serialize field;
    }) value;
  };

  bounded =
    value:
    if size 0 budget value > budget then
      {
        kind = "too-large";
        values = budget;
        depth = maxDepth;
      }
    else
      node value;
in
{
  inherit relative;
  # a declaration or manifest: each top-level field is counted on its own, so a field too large to
  # walk is reported at that field and the rest are still read.
  value = value: if isAttrs value && !isOpaque value then fields bounded value else bounded value;
}
