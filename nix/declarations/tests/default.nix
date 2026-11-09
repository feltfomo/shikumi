# nix-unit tests of the declaration layer, run by the gate's `declarations` check. they call
# `compose` directly on the fixture repositories beside this file. each refusal is written once, in
# `refusals`, which gives it a test named for the rule it holds and a place in the one-pass test, so
# removing a check fails the test named for it.
{ nixpkgs }:
let
  lib = import (nixpkgs + "/lib");
  shikumi = import ../. { inherit lib; };

  compose =
    root: focus:
    (shikumi.compose {
      inherit root focus;
      manifest = root + "/shikumi.nix";
    }).document;

  basic = compose ./fixtures/basic null;
  broken = compose ./fixtures/broken null;

  # a finding without its message, which is for people and free to change.
  located = finding: {
    inherit (finding)
      code
      option
      files
      project
      ;
  };
  findings = map located broken.diagnostics;

  finding = code: option: files: project: {
    inherit
      code
      option
      files
      project
      ;
  };
  manifest = code: option: finding code option [ "shikumi.nix" ];
  declaredIn =
    project: code: option:
    finding code option [ "projects/${project}/project.nix" ] project;
  twins =
    option:
    finding "duplicate" option [
      "projects/duplicate-a/project.nix"
      "projects/duplicate-b/project.nix"
    ] null;

  # every finding the broken fixture has to produce, in the order the composition reports them.
  refusals = [
    {
      rule = "AnUnknownManifestOption";
      finding = manifest "unknown-option" "owners" null;
    }
    {
      rule = "AnExtensionSchemaOutsideTheVocabulary";
      finding = manifest "invalid" "extensions.broken" null;
    }
    {
      rule = "AnExtensionSchemaBuiltFromInternalTypes";
      finding = manifest "invalid" "extensions.sneaky" null;
    }
    {
      rule = "ADirectoryOutsideTheRepository";
      finding = manifest "invalid" "projects.outside" null;
    }
    {
      rule = "ADirectoryInsideAnotherProject";
      finding = manifest "overlap" "projects.overlap" "overlap";
    }
    {
      rule = "AListedDirectoryWithoutADeclaration";
      finding = manifest "no-declaration" "projects.absent" "absent";
    }
    {
      rule = "AnInvalidIdentifier";
      finding = declaredIn "bad-names" "invalid" "project.dependsOn";
    }
    {
      rule = "ANameThatIsNotAnIdentifier";
      finding = declaredIn "bad-names" "invalid" "steps.Build";
    }
    {
      rule = "AValueOutsideAClosedEnumeration";
      finding = declaredIn "closed-enum" "enum" "services.worker.restart";
    }
    {
      rule = "ConflictingDefinitionsNamingBothFiles";
      finding = finding "conflict" "project.summary" [
        "projects/conflict/summary.nix"
        "projects/conflict/project.nix"
      ] "conflict";
    }
    {
      rule = "AnEmptyRequiredList";
      finding = declaredIn "empty-list" "invalid" "project.maintainers";
    }
    {
      rule = "AnExtensionNamespaceWithoutASchema";
      finding = declaredIn "extensions" "unknown-option" "extensions.nobody";
    }
    {
      rule = "AnExtensionNamespaceWhoseSchemaWasRefused";
      finding = declaredIn "extensions" "unknown-option" "extensions.sneaky";
    }
    {
      rule = "AGlobThatLeavesTheRepository";
      finding = declaredIn "extensions" "invalid" "extensions.layout.pages";
    }
    {
      rule = "APathInJsonData";
      finding = declaredIn "extensions" "type" "extensions.meta";
    }
    {
      rule = "AnExtensionValueItsSchemaRejects";
      finding = declaredIn "extensions" "invalid" "extensions.team.owner";
    }
    {
      rule = "AStorePathStrippedOfItsContext";
      finding = declaredIn "laundered" "store-reference" "project.summary";
    }
    {
      rule = "TheRepositoryLocationAsAJsonKey";
      finding = declaredIn "location" "location" "extensions.meta";
    }
    {
      rule = "TheRepositoryLocation";
      finding = declaredIn "location" "location" "project.summary";
    }
    {
      rule = "AMissingRequiredOption";
      finding = declaredIn "missing" "missing" "project.summary";
    }
    {
      rule = "AMissingRequiredFieldOfARecord";
      finding = declaredIn "missing" "missing" "steps.build.exec";
    }
    {
      rule = "AStoreReference";
      finding = declaredIn "store-reference" "store-reference" "project.summary";
    }
    {
      rule = "AnUnknownTopLevelOption";
      finding = declaredIn "unknown-options" "unknown-option" "tasks";
    }
    {
      rule = "AnUnknownField";
      finding = declaredIn "unknown-options" "unknown-option" "project.owner";
    }
    {
      rule = "AnUnknownFieldOfANamedRecord";
      finding = declaredIn "unknown-options" "unknown-option" "steps.lint.cmd";
    }
    {
      rule = "AValueOfTheWrongType";
      finding = declaredIn "wrong-type" "type" "project.maintainers";
    }
    {
      rule = "AListWhereAnAttributeSetBelongs";
      finding = declaredIn "wrong-type" "type" "steps";
    }
    {
      rule = "ADependencyOnAnUnlistedProject";
      finding = declaredIn "dangling" "unknown-reference" "project.dependsOn";
    }
    {
      rule = "OrderingAfterAMissingStep";
      finding = declaredIn "dangling" "unknown-reference" "steps.test.after";
    }
    {
      rule = "ACommandTwoProjectsDeclare";
      finding = twins "commands.fmt";
    }
    {
      rule = "AServiceTwoProjectsDeclare";
      finding = twins "services.db";
    }
  ];
in
lib.listToAttrs (
  map (refusal: {
    name = "testRefuses${refusal.rule}";
    value = {
      expr = builtins.elem refusal.finding findings;
      expected = true;
    };
  }) refusals
)
// {
  testValidRepositoryHasNoFindings = {
    expr = basic.diagnostics;
    expected = [ ];
  };

  testGlobalCompositionHoldsEveryProjectAndTheRemainder = {
    expr = {
      projects = builtins.attrNames basic.projects;
      inherit (basic) focus remainder;
    };
    expected = {
      projects = [
        "core"
        "docs"
        "web"
      ];
      focus = null;
      remainder.excludes = [
        "apps/web"
        "docs"
        "libs/core"
      ];
    };
  };

  testFocusedCompositionHoldsWhatTheProjectDependsOn = {
    expr =
      let
        focused = compose ./fixtures/basic "docs";
      in
      {
        projects = builtins.attrNames focused.projects;
        inherit (focused) focus remainder;
      };
    expected = {
      projects = [
        "core"
        "docs"
        "web"
      ];
      focus = "docs";
      remainder = null;
    };
  };

  testFocusedCompositionLeavesOutWhatNothingNames = {
    expr = builtins.attrNames (compose ./fixtures/basic "core").projects;
    expected = [ "core" ];
  };

  testFocusedCompositionReadsNoOtherDeclaration = {
    expr = builtins.attrNames (compose ./fixtures/lazy "leaf").projects;
    expected = [ "leaf" ];
  };

  testGlobalCompositionSurfacesADeclarationsOwnError = {
    expr = builtins.attrNames (compose ./fixtures/lazy null).projects;
    expectedError = {
      type = "ThrownError";
      msg = "the unrelated declaration was read";
    };
  };

  testDocumentCarriesNoCommandLine = {
    expr = {
      inherit (basic.projects.web.commands) serve;
      inherit (basic.projects.web.services) database;
      inherit (basic.projects.core.steps) test;
    };
    expected = {
      serve.description = "Serve the site locally";
      database = {
        description = "A local database for development";
        restart = "on-failure";
      };
      test = {
        description = "Run the unit tests";
        after = [ "build" ];
      };
    };
  };

  testDefinitionsMergeAcrossImportsAndPriorities = {
    expr = {
      services = builtins.attrNames basic.projects.web.services;
      steps = builtins.attrNames basic.projects.web.steps;
    };
    expected = {
      services = [ "database" ];
      steps = [ "bundle" ];
    };
  };

  testOptionalFieldsTakeThePublishedDefault = {
    expr = {
      inherit (basic.projects.core) dependsOn commands services;
      after = basic.projects.web.steps.bundle.after;
    };
    expected = {
      dependsOn = [ ];
      commands = { };
      services = { };
      after = [ ];
    };
  };

  testExtensionValuesPassThroughUnchanged = {
    expr = {
      core = basic.projects.core.extensions;
      web = basic.projects.web.extensions;
      docs = basic.projects.docs.extensions;
    };
    expected = {
      core.team = {
        owner = "platform";
        tier = "critical";
      };
      web.team.owner = "web";
      docs.layout.pages = [ "pages/**/*.md" ];
    };
  };

  testRefusedDocumentCarriesNoProjects = {
    expr = {
      inherit (broken) projects remainder;
    };
    expected = {
      projects = null;
      remainder = null;
    };
  };

  testEveryFindingComesBackInOnePass = {
    expr = findings;
    expected = map (refusal: refusal.finding) refusals;
  };

  testRefusesAFocusOnAnUnlistedProject = {
    expr =
      let
        document = compose ./fixtures/basic "nowhere";
      in
      {
        inherit (document) focus projects;
        findings = map located document.diagnostics;
      };
    expected = {
      focus = null;
      projects = null;
      findings = [
        {
          code = "unknown-reference";
          option = null;
          files = [ "shikumi.nix" ];
          project = null;
        }
      ];
    };
  };
}
