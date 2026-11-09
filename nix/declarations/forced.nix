# forces a value a declaration or the manifest wrote, inside a frame the producer reads. a failure
# inside the frame happened in the user's own code, whether or not the evaluator gives it a position
# there, since nothing of Shikumi's runs inside it; one outside every such frame, in Shikumi's own
# Nix, is a bug in Shikumi. every place the layer first touches a user's value goes through here.
# the frame's text is the layer table's, which the producer reads too. `addErrorContext` forces
# its value itself, so no call of Shikumi's own sits inside the frame.
let
  frame = (builtins.fromJSON (builtins.readFile ./layer.json)).frames.value;
in
value: builtins.addErrorContext frame value
