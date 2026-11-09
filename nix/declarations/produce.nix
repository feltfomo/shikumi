# what the producer evaluates through the host's `nix-instantiate`, once per request. every argument
# arrives as a string, so nothing the caller passes is parsed as Nix. `nixpkgs` is the source tree
# whose `lib` the layer runs on, and the only nixpkgs the evaluation sees. `request` is JSON, one of:
#
# - `{ "plan": { } }`: the serialized manifest, and each listed project's declaration file. no
#   declaration is read.
# - `{ "evaluate": [{ "name": <name>, "file": <file> }] }`: those projects' serialized declarations,
#   read from the files the plan gave them, in the order asked. the manifest isn't read.
# - `{ "locate": { "name": <name>, "file": <file> } }`: one project's serialized declaration, with
#   every string in it written out as JSON under a frame that names its field, so an evaluator that
#   can't write one says where it is.
#
# the answer is the `shikumi.evaluation` contract the producer reads.
{
  nixpkgs,
  root,
  request,
}:
let
  inherit (builtins)
    addErrorContext
    foldl'
    fromJSON
    seq
    toJSON
    ;

  shikumi = import ./. { lib = import (/. + nixpkgs + "/lib"); };
  checkout = /. + root;
  asked = fromJSON request;

  composed = shikumi.compose {
    root = checkout;
    manifest = checkout + "/shikumi.nix";
  };

  # the frame that names a project in a trace, around everything its answer forces. the name is
  # JSON, so any name reads back exactly; the producer reads this text, so the two change together.
  within = name: addErrorContext "shikumi-project ${toJSON name}";

  # every project is written out as JSON inside the frame that names it, so a failure anywhere in
  # its declaration, an invalid string included, names it. a string that isn't UTF-8 fails only
  # when it's written as JSON, which would otherwise happen outside every frame, after the whole
  # answer evaluated.
  evaluated =
    { name, file }:
    let
      entry = composed.read file;
    in
    within name (seq (toJSON entry) entry);

  # the frame that names a string's field: its path and the nearest position on it.
  strings =
    path: at: node:
    if node.kind == "string" then
      addErrorContext "shikumi-field ${toJSON { inherit path at; }}" (toJSON node.value)
    else if node.kind == "attrs" then
      foldl' (
        written: name:
        let
          field = node.fields.${name};
        in
        seq written (strings (path ++ [ name ]) (if field.at == null then at else field.at) field.value)
      ) "" (builtins.attrNames node.fields)
    else if node.kind == "list" then
      foldl' (
        written: index: seq written (strings (path ++ [ index ]) at (builtins.elemAt node.items index))
      ) "" (builtins.genList (index: index) (builtins.length node.items))
    else
      "";

  located =
    { name, file }:
    let
      entry = composed.read file;
    in
    within name (seq (strings [ ] null entry.value) entry);

in
{
  schema = "shikumi.evaluation";
  version = {
    major = 1;
    minor = 0;
  };
  answer =
    if asked ? plan then
      {
        plan = {
          inherit (composed) manifest files;
        };
      }
    else if asked ? evaluate then
      { projects = map evaluated asked.evaluate; }
    else
      { projects = [ (located asked.locate) ]; };
}
