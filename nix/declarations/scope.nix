# the scope every file a declaration's evaluation reaches is evaluated in: each user file, each file
# one of them imports, and nixpkgs, whose `lib` and package set a user file is handed and whose own
# importers, `callPackage` and `evalModules` among them, take a user's path. its `builtins` lacks
# the builtins the layer table withholds, which read outside the keyed files whatever pure and
# restricted mode allow: Determinate Nix's `getFlake` takes a locked reference to any flake, commit
# or directory on the host. `import`, `scopedImport` and `builtins` itself are replaced, so a file
# reached any of those ways is evaluated in the scope too, and the global names an evaluator gives
# a withheld builtin are shadowed, since a scope can add names but not remove them.
#
# `scopedImport` evaluates a file again on every import, where `import` evaluates it once (cppnix
# 2.35.3 src/libexpr/primops.cc:266-288 against src/libexpr/eval.cc:1147-1175, determinate 3.24.2
# primops.cc:270-289 against eval.cc:1198-1235, lix 2.95.4 lix/libexpr/primops.cc:234-260 against
# eval.cc:939-975, lix 2.96.0 primops.cc:240-264 against eval.cc:919-952), and nixpkgs imports many
# of its files more than once. so a file under nixpkgs or the repository's copy is imported through
# a lazy mirror of its tree, one `readDir` per directory a path goes through and one value per
# file, as `import` itself keeps one. any other path is imported in the scope without the mirror.
{ nixpkgs, withheld }:
let
  real = builtins;
  names = map (builtin: builtin.name) withheld;
  kept = removeAttrs real names;

  # reaching a withheld builtin by any name gives the evaluator's own missing-attribute error,
  # which the producer reads as a refused read.
  shadows = real.listToAttrs (
    real.concatMap (
      builtin:
      [
        {
          name = "__${builtin.name}";
          value = kept.${builtin.name};
        }
      ]
      ++ (
        if builtin.global then
          [
            {
              inherit (builtin) name;
              value = kept.${builtin.name};
            }
          ]
        else
          [ ]
      )
    ) withheld
  );

  # a lookup-path entry whose path names a scheme, `flake:`, `channel:` or a URL, reaches the
  # fetchers `fetchTree` stands for, through `findFile` or `<...>`, which a declaration can give
  # its own `__nixPath`. where `fetchTree` is withheld such an entry is refused the same way;
  # nixpkgs' own `<nix/fetchurl.nix>` still resolves, since every implementation finds a `nix/`
  # path among its own files once no lookup-path entry holds it (cppnix 2.35.3
  # src/libexpr/eval.cc:3306-3307, determinate 3.24.2 eval.cc:3509-3510, lix 2.95.4
  # lix/libexpr/eval.cc:2180-2181, lix 2.96.0 eval.cc:2208-2209).
  findFile =
    lookupPath: name:
    if
      real.elem "fetchTree" names
      && real.any (
        entry: real.match "[A-Za-z][A-Za-z0-9+.-]*:.*" (toString (entry.path or "")) != null
      ) lookupPath
    then
      kept.fetchTree
    else
      real.findFile lookupPath name;

  # the scope whose `import` is `importing`. a file `scopedImport`s with names of its own, but
  # never with its own `builtins` or `import`.
  scopeOf =
    importing:
    let
      scope = shadows // {
        builtins = kept // {
          inherit (scope) builtins import scopedImport;
          inherit findFile;
        };
        import = importing;
        scopedImport = extra: real.scopedImport (extra // scope);
        __findFile = findFile;
      };
    in
    scope;

  uncached = real.scopedImport (scopeOf uncached);

  # the tree under `directory`: each directory's entries, and each regular file's value, imported
  # in `scope` when a path first reaches it.
  mirror =
    scope: directory:
    real.mapAttrs (
      name: type:
      if type == "directory" then
        { entries = mirror scope (directory + "/${name}"); }
      else if type == "regular" then
        { value = real.scopedImport scope (directory + "/${name}"); }
      else
        { }
    ) (real.readDir directory);

  walk =
    node: parts:
    if parts == [ ] then
      node
    else if node ? entries && node.entries ? ${real.head parts} then
      walk node.entries.${real.head parts} (real.tail parts)
    else
      { };

  # a path as text to compare and split: Determinate Nix gives a store path's text the path as
  # context (determinate 3.24.2 src/libexpr/eval.cc:2658-2673), which an attribute name can't
  # carry.
  textOf = path: real.unsafeDiscardStringContext (toString path);

  # a tree under `base` whose files are imported in the scope of `importing`.
  tree = importing: base: {
    base = textOf base;
    root.entries = mirror (scopeOf importing) base;
  };

  # an import function that keeps each file under one of `trees` once, and imports anything else
  # in the scope without a mirror.
  cachedIn =
    trees: path:
    let
      text = textOf path;
      under =
        tree:
        text == tree.base || real.substring 0 (real.stringLength tree.base + 1) text == "${tree.base}/";
      parts =
        tree:
        real.filter real.isString (
          real.split "/" (real.substring (real.stringLength tree.base + 1) (-1) text)
        );
      found = real.foldl' (
        found: tree: if found == null && under tree then walk tree.root (parts tree) else found
      ) null trees;
      file = if found != null && found ? entries then found.entries."default.nix" or { } else found;
    in
    if file != null && file ? value then file.value else uncached path;

  nixpkgsTree = tree importNixpkgs nixpkgs;
  importNixpkgs = cachedIn [ nixpkgsTree ];
in
{
  # imports a file of nixpkgs, or any other, in the scope.
  import = importNixpkgs;

  # the import for a repository whose copy is at `root`, keeping its files and nixpkgs' once each.
  importIn =
    root:
    let
      importing = cachedIn [
        (tree importing root)
        nixpkgsTree
      ];
    in
    importing;
}
