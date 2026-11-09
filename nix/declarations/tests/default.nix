# nix-unit tests of the declaration layer, run by the gate's `declarations` check: the factory,
# serialization and its source positions, the vocabulary, composition over the fixture repositories
# beside this file, and the module adapter in each module system. whether a declaration is right is
# the Rust checker's to say, so nothing here asserts a finding; `testNothingIsCheckedInNix` holds
# that the layer passes every value through as written.
{
  nixpkgs,
  # Home Manager's source, as nixpkgs pins it for its `home-manager` package.
  homeManager,
  # what the vocabulary's constructors build for `vocabulary.nix`, tracked beside the checker.
  vocabulary,
  # the registry crate's tracked references, which hold its copy of the layer table.
  references,
}:
let
  lib = import (nixpkgs + "/lib");
  shikumi = import ../. { inherit lib; };
  inherit (shikumi) types;

  composition =
    root:
    shikumi.compose {
      inherit root;
      manifest = root + "/shikumi.nix";
    };

  # the manifest and every listed project's entry, as the producer assembles a global composition.
  compose =
    root:
    let
      composed = composition root;
    in
    {
      inherit (composed) manifest;
      projects = builtins.mapAttrs (
        _: file: if file == null then null else composed.read file
      ) composed.files;
    };

  basic = compose ./fixtures/basic;
  broken = compose ./fixtures/broken;

  serialize = import ../serialize.nix {
    inherit lib;
    forced = import ../forced.nix;
  };
  # serialization relative to this directory, so positions name files beside this one.
  serialized = (serialize (toString ./.)).value;

  at = file: line: column: { inherit file line column; };

  # a serialized attribute set's fields, each without its position.
  valuesOf = node: builtins.mapAttrs (_: field: field.value) node.fields;

  # the value a declaration wrote, in the terms serialization keeps: text without its context,
  # paths relative to `root`, and functions and derivations by their kind. it's written apart from
  # `serialize.nix`, so a change there that alters a value shows up as a difference.
  written =
    root: value:
    if builtins.isString value then
      builtins.unsafeDiscardStringContext value
    else if builtins.isPath value then
      lib.removePrefix "${root}/" (toString value)
    else if builtins.isList value then
      map (written root) value
    else if builtins.isFunction value then
      "<function>"
    else if lib.isDerivation value then
      "<derivation>"
    else if builtins.isAttrs value then
      builtins.mapAttrs (_: written root) value
    else
      value;

  # a serialized node read back into the same terms.
  read =
    node:
    if node.kind == "attrs" then
      builtins.mapAttrs (_: field: read field.value) node.fields
    else if node.kind == "list" then
      map read node.items
    else if node.kind == "function" then
      "<function>"
    else if node.kind == "derivation" then
      "<derivation>"
    else if node.kind == "null" then
      null
    else
      node.value;

  # a declaration file of the broken fixture, evaluated on its own, outside composition.
  writtenIn =
    directory:
    let
      file = import (./fixtures/broken + "/${directory}/project.nix");
      value =
        if builtins.isFunction file then
          file (builtins.intersectAttrs (builtins.functionArgs file) { inherit lib shikumi; })
        else
          file;
    in
    written (toString ./fixtures/broken) (value.declaration or value);

  web = shikumi.project {
    project = {
      summary = "The web front end";
      maintainers = [ "web-team" ];
    };
    commands.serve = {
      description = "Serve the site";
      exec = "serve";
    };
  };
  api = shikumi.project {
    project = {
      summary = "The API";
      maintainers = [ "api-team" ];
    };
  };

  # what a module system has to hand back: the same value, with the positions its record had.
  carried = result: {
    same = result == web;
    positions = serialized result.declaration == serialized web.declaration;
  };
  unchanged = {
    same = true;
    positions = true;
  };

  pkgs = import nixpkgs {
    system = "x86_64-linux";
    config = { };
    overlays = [ ];
  };

  flakeParts = import ./flake-parts.nix { inherit lib; };
in
{
  testTheFactoryMarksTheRecordAndChangesNothingElse = {
    expr = shikumi.project { project.summary = "As written"; };
    expected = {
      _type = "shikumi-project";
      declaration.project.summary = "As written";
    };
  };

  testEveryKindOfValueSerializesAsWhatItWas = {
    expr = valuesOf (serialized {
      text = "words";
      number = 3;
      ratio = 0.5;
      flag = true;
      nothing = null;
      inside = ./fixtures/basic/shikumi.nix;
      outside = /nowhere/at/all;
      function = x: x;
      package = derivation {
        name = "package";
        system = "x86_64-linux";
        builder = "/bin/sh";
      };
    });
    expected = {
      text = {
        kind = "string";
        value = "words";
        context = false;
      };
      number = {
        kind = "int";
        value = 3;
      };
      ratio = {
        kind = "float";
        value = 0.5;
      };
      flag = {
        kind = "bool";
        value = true;
      };
      nothing.kind = "null";
      inside = {
        kind = "path";
        value = "fixtures/basic/shikumi.nix";
      };
      outside = {
        kind = "path";
        value = null;
      };
      function.kind = "function";
      package.kind = "derivation";
    };
  };

  # the producer prints the document as it stands, so nothing in a whole composition may carry a
  # store reference, though the broken fixture's declarations write several.
  testAComposedDocumentCarriesNoStoreReference = {
    expr = map (document: builtins.hasContext (builtins.toJSON document)) [
      basic
      broken
    ];
    expected = [
      false
      false
    ];
  };

  testAStringThatRefersToTheStoreKeepsItsTextAndFlagsItsContext = {
    expr =
      let
        package = derivation {
          name = "package";
          system = "x86_64-linux";
          builder = "/bin/sh";
        };
        node = serialized "built by ${package}";
      in
      {
        inherit (node) context;
        hasContext = builtins.hasContext node.value;
        text = node.value == "built by ${builtins.unsafeDiscardStringContext package.outPath}";
      };
    expected = {
      context = true;
      hasContext = false;
      text = true;
    };
  };

  # a value that shares one value at every level is finite to Nix and exponential to walk, and a
  # value that refers to itself is endless; each top-level field holding one is cut there, alone.
  testAFieldTooLargeToWalkIsCutAtThatFieldAlone = {
    expr =
      let
        shared = {
          a = shared;
          b = shared;
        };
        chain = {
          inner = chain;
        };
        node = serialized {
          inherit shared chain;
          kept = [ 1 ];
        };
      in
      builtins.mapAttrs (_: field: field.value.kind) node.fields;
    expected = {
      shared = "too-large";
      chain = "too-large";
      kept = "list";
    };
  };

  # the checker states the limits a value broke from what the node carries, so they have one source.
  testAValueTooLargeToWalkCarriesTheLimitsItBroke = {
    expr =
      let
        chain = {
          inner = chain;
        };
      in
      (serialized { inherit chain; }).fields.chain.value;
    expected = {
      kind = "too-large";
      values = 100000;
      depth = 32;
    };
  };

  testANumberThatIsNotFiniteSerializesAsSuch = {
    expr = map (value: (serialized value).kind) [
      (1.0e308 * 10.0)
      0.5
    ];
    expected = [
      "non-finite"
      "float"
    ];
  };

  # what composition reads is guarded, and only that: a directory where the declaration file
  # belongs isn't imported, a file that asks for an argument it isn't given isn't called, and a
  # value that carries the factory's mark without its record isn't taken for a declaration.
  testCompositionReadsOnlyWhatItCanReadWithoutFailing = {
    expr =
      let
        inherit (compose ./fixtures/odd) projects;
      in
      {
        inherit (projects) dir;
        args = {
          inherit (projects.args) factory;
          inherit (projects.args.value) kind;
        };
        fake = projects.fake.factory;
      };
    expected = {
      dir = null;
      args = {
        factory = false;
        kind = "function";
      };
      fake = false;
    };
  };

  testEveryAttributeCarriesThePositionThatDefinedIt = {
    expr =
      let
        node = serialized (import ./serialization/value.nix { inherit lib; });
        field = path: lib.foldl' (node: name: node.value.fields.${name}) { value = node; } path;
      in
      {
        project = (field [ "project" ]).at;
        summary =
          (field [
            "project"
            "summary"
          ]).at;
        dotted =
          (field [
            "steps"
            "build"
            "description"
          ]).at;
        imported =
          (field [
            "commands"
            "fmt"
          ]).at;
        # a list's elements have no position of their own.
        listItems = builtins.attrNames (
          builtins.head
            (field [
              "project"
              "tags"
            ]).value.items
        );
        # `builtins.mapAttrs` gives its attributes none.
        mapped =
          (field [
            "mapped"
            "inner"
          ]).at;
        # nixpkgs' `genAttrs` gives its attributes a position inside nixpkgs, outside the
        # repository, which is no position the repository can show.
        generated =
          (field [
            "generated"
            "inner"
          ]).at;
      };
    expected = {
      project = at "serialization/value.nix" 5 3;
      summary = at "serialization/value.nix" 6 5;
      dotted = at "serialization/value.nix" 12 3;
      imported = at "serialization/part.nix" 1 3;
      listItems = [
        "context"
        "kind"
        "value"
      ];
      mapped = null;
      generated = null;
    };
  };

  testTheVocabularyBuildsTheTrackedSchemaData = {
    expr = import ./vocabulary.nix types;
    expected = builtins.fromJSON (builtins.readFile vocabulary);
  };

  testCompositionGivesTheManifestEachFileAndEachProjectApart = {
    expr =
      let
        composed = composition ./fixtures/basic;
      in
      {
        fields = builtins.attrNames composed;
        manifest = composed.manifest.file;
        inherit (composed) files;
      };
    expected = {
      fields = [
        "files"
        "manifest"
        "read"
      ];
      manifest = "shikumi.nix";
      files = {
        core = "libs/core/project.nix";
        docs = "docs/project.nix";
        web = "apps/web/project.nix";
      };
    };
  };

  # the producer reads the manifest once and then each project on its own, so naming the files
  # reads no declaration, and reading one project reads no other.
  testNamingTheFilesReadsNoDeclaration = {
    expr =
      let
        composed = composition ./fixtures/lazy;
      in
      {
        files = builtins.attrNames composed.files;
        leaf = builtins.deepSeq (composed.read composed.files.leaf) true;
      };
    expected = {
      files = [
        "leaf"
        "unrelated"
      ];
      leaf = true;
    };
  };

  # nothing is checked in Nix: every declaration of the broken fixture, wrong as each one is, comes
  # out of composition exactly as its file wrote it, with nothing dropped, defaulted, replaced or
  # added, and an entry whose file can't be read comes out as null rather than as a finding. a
  # check planted anywhere in the layer that refuses, rewrites or filters a value fails this test;
  # one that throws fails it too, and one that adds a field fails the contract the Rust checker
  # reads, which refuses unknown fields.
  testNothingIsCheckedInNix = {
    expr = builtins.mapAttrs (
      _: declaration: if declaration == null then null else read declaration.value
    ) broken.projects;
    expected =
      let
        listed = (import ./fixtures/broken/shikumi.nix { inherit types; }).projects;
      in
      builtins.mapAttrs (
        project: directory:
        if
          builtins.elem project [
            "absent"
            "outside"
          ]
        then
          null
        else
          writtenIn directory
      ) listed;
  };

  testTheManifestPassesThroughAsWritten = {
    expr = read broken.manifest.value;
    expected = written (toString ./fixtures/broken) (
      import ./fixtures/broken/shikumi.nix { inherit types; }
    );
  };

  testARecordThatSkippedTheFactoryIsMarkedAsSuch = {
    expr = broken.projects.no-factory.factory;
    expected = false;
  };

  testGlobalCompositionHoldsEveryProject = {
    expr = builtins.attrNames basic.projects;
    expected = [
      "core"
      "docs"
      "web"
    ];
  };

  # a project read from the file its selection named gives the entry composition gives, without
  # the manifest: here, one that throws as soon as it's imported.
  testReadingAProjectFromItsFileNeedsNoManifest = {
    expr =
      let
        composed = shikumi.compose {
          root = ./fixtures/basic;
          manifest = throw "the manifest was read";
        };
      in
      composed.read "libs/core/project.nix" == basic.projects.core;
    expected = true;
  };

  testGlobalCompositionSurfacesADeclarationsOwnError = {
    expr = builtins.deepSeq (compose ./fixtures/lazy) true;
    expectedError = {
      type = "ThrownError";
      msg = "the unrelated declaration was read";
    };
  };

  testADeclarationSplitWithImportKeepsEachFilesPositions = {
    expr =
      let
        core = basic.projects.core.value.fields;
      in
      {
        project = core.project.at.file;
        steps = core.steps.at.file;
      };
    expected = {
      project = "libs/core/project.nix";
      steps = "libs/core/steps.nix";
    };
  };

  testNixOSCarriesAFactoryResultUnchanged = {
    expr =
      let
        system = import (nixpkgs + "/nixos/lib/eval-config.nix") {
          system = "x86_64-linux";
          modules = [
            shikumi.modules.nixos
            { shikumi.projects.web = web; }
          ];
        };
      in
      carried system.config.shikumi.projects.web;
    expected = unchanged;
  };

  testHomeManagerCarriesAFactoryResultUnchanged = {
    expr =
      let
        home = import (homeManager + "/modules") {
          inherit pkgs;
          configuration = {
            imports = [ shikumi.modules.homeManager ];
            shikumi.projects.web = web;
            home = {
              username = "someone";
              homeDirectory = "/home/someone";
              stateVersion = "25.05";
            };
          };
        };
      in
      carried home.config.shikumi.projects.web;
    expected = unchanged;
  };

  testFlakePartsCarriesFactoryResultsUnchangedAtTheTopAndPerSystem = {
    expr =
      let
        flake = flakeParts.evaluate [
          shikumi.modules.flakeParts
          {
            systems = [ "x86_64-linux" ];
            shikumi.projects.web = web;
            perSystem = _: { shikumi.projects.api = api; };
          }
        ];
      in
      {
        top = carried flake.config.shikumi.projects.web;
        perSystem = flake.config.allSystems.x86_64-linux.shikumi.projects.api == api;
      };
    expected = {
      top = unchanged;
      perSystem = true;
    };
  };

  testAModuleSystemRefusesAValueThatIsntAFactoryResult = {
    expr =
      (lib.evalModules {
        modules = [
          shikumi.modules.nixos
          { shikumi.projects.web.project.summary = "Not through the factory"; }
        ];
      }).config.shikumi.projects.web;
    expectedError.type = "ThrownError";
  };

  testAModuleSystemRefusesASecondDefinitionRatherThanMergeIt = {
    expr =
      (lib.evalModules {
        modules = [
          shikumi.modules.nixos
          { shikumi.projects.web = web; }
          { shikumi.projects.web = api; }
        ];
      }).config.shikumi.projects.web;
    expectedError.type = "ThrownError";
  };

  # the layer reads its own copy of the table the registry's Rust defines.
  testTheLayerReadsTheRegistrysTable = {
    expr = builtins.fromJSON (builtins.readFile ../layer.json);
    expected = builtins.fromJSON (builtins.readFile (references + "/layer.json"));
  };

  # the files are offered exactly the arguments the layer table names, which the reference page and
  # the checker's messages name too.
  testEachFileIsOfferedTheArgumentsTheLayerTableNames =
    let
      layer = builtins.fromJSON (builtins.readFile ../layer.json);
      composed = compose ./fixtures/arguments;
    in
    {
      expr = {
        manifest = read composed.manifest.value.fields.offered.value;
        declaration = read composed.projects.app.value;
      };
      expected = {
        manifest = layer.manifest.arguments;
        declaration = layer.declaration.arguments;
      };
    };
}
