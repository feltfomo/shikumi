# whether a withheld builtin is there to call, as the file that asks is evaluated.
builtins ? getFlake || builtins ? fetchTree || builtins.builtins ? getFlake
