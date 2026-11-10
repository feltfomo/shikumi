# a flake outside the repository, which a planted declaration reads by a locked reference.
{ outputs = _: { secret = "Read through a flake reference"; }; }
