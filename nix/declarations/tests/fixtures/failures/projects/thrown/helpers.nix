{
  required = name: value: if value == null then throw "${name} is required" else value;
}
