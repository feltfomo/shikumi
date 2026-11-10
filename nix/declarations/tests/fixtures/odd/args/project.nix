# asks for an argument a declaration isn't given.
{ shikumi, pkgs }: shikumi.project { project.summary = pkgs.hello.pname; }
