# the parts of flake-parts the module adapter touches, copied so the adapter's test runs against
# flake-parts' own option declarations without a third Nix input: the top-level `systems` option,
# the `perSystem` option and its deferred-module type, and `allSystems`, which exposes each
# system's `perSystem` configuration. copied from flake-parts at revision
# ea2630b7470978622c7689a1ff75d2b389175399, `lib.nix` and `modules/perSystem.nix`, leaving out what
# the adapter doesn't reach: `perInput`, the `inputs'` and `self'` arguments, and the alias errors.
#
# MIT License
#
# Copyright (c) 2021 Hercules CI
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in all
# copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
# SOFTWARE.
{ lib }:
let
  inherit (lib)
    mkOption
    mkOptionType
    defaultFunctor
    isAttrs
    isFunction
    showOption
    types
    genAttrs
    ;
  inherit (lib.types)
    path
    submoduleWith
    ;

  # from `lib.nix`.
  deferredModuleWith =
    attrs@{
      staticModules ? [ ],
    }:
    mkOptionType {
      name = "deferredModule";
      description = "module";
      check = x: isAttrs x || isFunction x || path.check x;
      merge =
        loc: defs:
        staticModules
        ++ map (
          def: lib.setDefaultModuleLocation "${def.file}, via option ${showOption loc}" def.value
        ) defs;
      inherit (submoduleWith { modules = staticModules; })
        getSubOptions
        getSubModules
        ;
      substSubModules =
        m:
        deferredModuleWith (
          attrs
          // {
            staticModules = m;
          }
        );
      functor = defaultFunctor "deferredModuleWith" // {
        type = deferredModuleWith;
        payload = {
          inherit staticModules;
        };
        binOp = lhs: rhs: {
          staticModules = lhs.staticModules ++ rhs.staticModules;
        };
      };
    };

  mkLegacyDeferredModuleType =
    module:
    deferredModuleWith {
      staticModules = [ module ];
    };

  flake-parts-lib = rec {
    mkPerSystemType = mkLegacyDeferredModuleType;
    mkDeferredModuleOption =
      module:
      mkOption {
        type = flake-parts-lib.mkPerSystemType module;
      };
    mkPerSystemOption = mkDeferredModuleOption;
  };

  # from `modules/perSystem.nix`.
  perSystem =
    { config, ... }:
    {
      options = {
        systems = mkOption {
          type = types.listOf types.str;
        };
        perSystem = mkOption {
          type = flake-parts-lib.mkPerSystemType (_: {
            _file = ./flake-parts.nix;
          });
          apply =
            modules: system:
            (lib.evalModules {
              inherit modules;
              prefix = [
                "perSystem"
                system
              ];
              specialArgs = {
                inherit system;
              };
              class = "perSystem";
            }).config;
        };
        allSystems = mkOption {
          type = types.lazyAttrsOf types.unspecified;
          internal = true;
        };
      };
      config.allSystems = genAttrs config.systems config.perSystem;
    };
in
{
  # a flake-parts evaluation of `modules`, as `evalFlakeModule` in `lib.nix` sets one up:
  # `flake-parts-lib` as a special argument and the `flake` class. its `self`, `inputs` and
  # `moduleLocation` arguments are left out, since the adapter reads none of them.
  evaluate =
    modules:
    lib.evalModules {
      specialArgs = { inherit flake-parts-lib; };
      class = "flake";
      modules = [ perSystem ] ++ modules;
    };
}
