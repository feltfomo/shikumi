# `shikumi.project`, the factory every declaration goes through. it marks the record it's given and
# changes nothing else: no merging, no defaults and no checks, so what a project writes is exactly
# what Shikumi reads. the record stays the same value, so the source positions of its attributes
# survive into serialization, through a module system's options as much as through `import` and `//`.
{
  project = declaration: {
    _type = "shikumi-project";
    inherit declaration;
  };

  isProject =
    value: builtins.isAttrs value && value._type or null == "shikumi-project" && value ? declaration;
}
