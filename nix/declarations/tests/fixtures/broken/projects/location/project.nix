{
  project = {
    summary = "Lives at ${toString ./.}";
    maintainers = [ "someone" ];
  };
  extensions.meta.${toString ./.} = "keyed by the checkout's location";
}
