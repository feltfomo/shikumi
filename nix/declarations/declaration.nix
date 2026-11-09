# one project's declaration: a module in devenv's flat shape, evaluated by the module system and
# checked by the schema. the module system collects definitions, follows `imports` and discharges
# `mkIf`, `mkMerge` and priorities; it never type-checks here. every option takes any value and the
# schema checks all of them afterwards, so a bad definition becomes a finding instead of an
# evaluation error that would hide every other one.
{ lib, schema }:
let
  inherit (schema.types)
    text
    identifier
    enum
    listOf
    nonEmptyListOf
    attrsOf
    record
    required
    optional
    ;
  inherit (schema.internal) command withDefault;

  # an undeclared top-level key lands in the freeform part, where this type records only the files
  # that set it, so the schema can report each one as an unknown option.
  unknownKey = lib.mkOptionType {
    name = "shikumi-unknown";
    merge = _: defs: { files = map (def: def.file) defs; };
  };

  options = {
    _file = "shikumi declaration options";
    options = lib.genAttrs names (
      _:
      lib.mkOption {
        type = lib.types.lazyAttrsOf lib.types.raw;
        description = "checked by Shikumi's declaration schema after evaluation";
      }
    );
    config._module.freeformType = lib.types.attrsOf unknownKey;
  };

  # what a declaration may say. `exec` is declared here and never copied into the document.
  type =
    extensions:
    record {
      project = required (record {
        summary = required text;
        maintainers = required (nonEmptyListOf text);
        dependsOn = withDefault [ ] (listOf identifier);
      });
      steps = withDefault { } (
        attrsOf (record {
          description = required text;
          exec = required command;
          after = withDefault [ ] (listOf identifier);
        })
      );
      commands = withDefault { } (
        attrsOf (record {
          description = required text;
          exec = required command;
        })
      );
      services = withDefault { } (
        attrsOf (record {
          description = required text;
          exec = required command;
          restart = required (enum [
            "never"
            "on-failure"
            "always"
          ]);
        })
      );
      extensions = withDefault { } (record (lib.mapAttrs (_: optional) extensions));
    };

  # the sections a declaration has, read off the record so a new one is declared in one place.
  names = builtins.attrNames (type { }).fields;
in
{
  # the declaration record, given the repository's extension schemas.
  inherit type;

  # checks the declaration at `file` against `declarationType`, which the composition builds once.
  # the file stands as an empty definition too, so a field nothing defines is reported against it.
  check =
    {
      ctx,
      declarationType,
      file,
    }:
    let
      evaluated = lib.evalModules {
        modules = [
          options
          file
        ];
      };
      declared = builtins.concatMap (
        name:
        map (def: {
          inherit (def) file;
          value.${name} = def.value;
        }) evaluated.options.${name}.definitionsWithLocations
      ) names;
      # a key the declaration declared an option for itself isn't unknown; it just isn't Shikumi's.
      unknown = lib.filterAttrs (
        name: _: name != "_module" && !(evaluated.options ? ${name})
      ) evaluated.config;
      undeclared = builtins.concatMap (
        name:
        map (definingFile: {
          file = definingFile;
          value.${name} = null;
        }) unknown.${name}.files
      ) (builtins.attrNames unknown);
    in
    schema.check ctx [ ] declarationType (
      [
        {
          file = toString file;
          value = { };
        }
      ]
      ++ declared
      ++ undeclared
    );
}
