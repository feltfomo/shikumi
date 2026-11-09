{
  services.database = {
    description = "A local database for development";
    exec = "database --data ./state";
    restart = "on-failure";
  };
}
