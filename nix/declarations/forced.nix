# forces a value a declaration or the manifest wrote, inside a frame the producer reads. a failure
# inside the frame happened in the user's own code, whether or not the evaluator gives it a position
# there, since nothing of Shikumi's runs inside it; one outside every such frame, in Shikumi's own
# Nix, is a bug in Shikumi. every place the layer first touches a user's value goes through here.
# the producer reads this text, so the two change together. `addErrorContext` forces its value
# itself, so no call of Shikumi's own sits inside the frame.
value: builtins.addErrorContext "shikumi-value" value
