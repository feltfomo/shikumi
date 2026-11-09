# a schema that uses every constructor in `shikumi.lib.types`, and the names `shikumi.lib.types`
# has. both are tracked as `crates/registry/fixtures/vocabulary.json`, which the checker's own tests
# read, so the Nix half of the vocabulary and the Rust half that reads and documents it can't drift
# apart.
types: {
  names = builtins.attrNames types;
  schema = types.record {
    everything = types.required (
      types.nonEmptyListOf (
        types.attrsOf (
          types.enum [
            "a"
            "b"
          ]
        )
      )
    );
    scalars = types.optional (
      types.listOf (
        types.record {
          glob = types.optional types.glob;
          identifier = types.optional types.identifier;
          json = types.optional types.json;
          path = types.optional types.path;
          text = types.optional types.text;
        }
      )
    );
  };
}
