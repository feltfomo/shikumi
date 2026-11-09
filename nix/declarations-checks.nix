# the gate's checks on the declaration layer and the registry document. each runs in the Nix build
# sandbox, where there's no daemon: the producer evaluates against `dummy://`, which needs none, and
# nix-unit and the control evaluations get a store of their own under the build's temporary
# directory. each check takes only the files it reads, so editing a unit test reruns `declarations`
# alone.
{
  pkgs,
  # the packaged `shikumi`, which runs the declaration layer it ships.
  shikumi,
  nix-unit,
  # the nixpkgs source tree the layer runs on.
  nixpkgs,
  # the declaration layer as `shikumi` ships it, for the control evaluations.
  layer,
  # all of nix/declarations, since the unit tests import the layer from beside them.
  tests,
  # what the vocabulary's constructors build for the unit tests' schema, tracked beside the
  # checker that reads it.
  vocabulary,
  # the fixture repositories, the planted cases and the worktree leak, each on its own.
  fixtures,
  planted,
  worktreeLeak,
  # the tracked references for each fixture, beside the registry crate whose tests read them too.
  # every implementation has to write exactly these bytes, which keeps them identical to each
  # other and to what the crate's tests check.
  references,
  # the tracked declarations reference page.
  referencePage,
}:
let
  inherit (pkgs) lib;

  # every implementation the producer has to keep its refusals on, as the conformance checks name
  # them.
  implementations = {
    cppnix-2-35 = pkgs.nixVersions.nix_2_35;
    determinate-3-24 = import ./determinate.nix { inherit pkgs; };
    lix-2-95 = pkgs.lixPackageSets.lix_2_95.lix;
    lix-2-96 = pkgs.lixPackageSets.lix_2_96.lix;
  };

  # what every run needs: Nix keeps its caches under the build's own directory, since the sandbox
  # has no home.
  prelude = ''
    set -euo pipefail
    export XDG_CACHE_HOME="$TMPDIR/cache"
  '';
in
{
  declarations =
    pkgs.runCommand "declarations"
      {
        nativeBuildInputs = [ nix-unit ];
      }
      ''
        ${prelude}
        nix-unit --eval-store "$TMPDIR/store" --option store "$TMPDIR/store" \
          --option allow-import-from-derivation false \
          ${tests}/tests/default.nix --argstr nixpkgs ${nixpkgs} \
          --argstr homeManager ${pkgs.home-manager.src} --argstr vocabulary ${vocabulary}
        touch $out
      '';

  # the reference page is generated from the vocabulary the checker checks with, and fails here
  # when the tracked page says anything else.
  declarations-reference =
    pkgs.runCommand "declarations-reference"
      {
        nativeBuildInputs = [
          shikumi
          pkgs.diffutils
        ];
      }
      ''
        shikumi registry reference > generated.md
        diff -u ${referencePage} generated.md || {
          echo "docs/reference/declarations.md is stale; regenerate it with shikumi registry reference" >&2
          exit 1
        }
        touch $out
      '';

  # the CLI end to end, on one implementation: the serialized declarations are what differ between
  # implementations, and the `registry-no-builds-*` checks hold those on each. this holds the exit
  # codes, where each output goes, and the rendered findings as a person sees them.
  registry-cli =
    pkgs.runCommand "registry-cli"
      {
        nativeBuildInputs = [
          shikumi
          pkgs.nixVersions.nix_2_35
          pkgs.diffutils
        ];
      }
      ''
        ${prelude}
        mkdir -p $out

        # runs shikumi with stdout and stderr in $out/$1, failing unless it exits with $2.
        run() {
          local name=$1 expected=$2 status=0
          shift 2
          shikumi "$@" > $out/$name 2> $out/$name.stderr || status=$?
          if [ "$status" -ne "$expected" ]; then
            echo "shikumi $* exited $status, not $expected:" >&2
            cat $out/$name.stderr >&2
            exit 1
          fi
        }

        run basic.document.json 0 registry document --root ${fixtures}/basic
        diff -u ${references}/basic/document.json $out/basic.document.json
        echo "basic: accepted, with the tracked document on stdout"

        # the check view prints what it was asked for, the findings, on stdout.
        run broken.findings.txt 1 registry check --root ${fixtures}/broken
        diff -u ${references}/broken/findings.txt $out/broken.findings.txt
        grep -q 'the declarations are refused, with' $out/broken.findings.txt.stderr

        # every other view renders the findings on stderr, prints nothing, and fails.
        run broken.list 1 registry list --root ${fixtures}/broken
        [ ! -s $out/broken.list ]
        diff -u ${references}/broken/findings.txt <(grep -v '^shikumi: ' $out/broken.list.stderr)
        grep -q '^shikumi: the declarations are refused, with' $out/broken.list.stderr
        echo "broken: refused, with the tracked findings where each view puts them"

        # declarations that fail to evaluate are findings like any other, each at the user's line,
        # beside the findings of the declarations that evaluated.
        run failures.findings.txt 1 registry check --root ${fixtures}/failures
        diff -u ${references}/failures/findings.txt $out/failures.findings.txt
        run failures.document.json 1 registry check --format json --root ${fixtures}/failures
        diff -u ${references}/failures/document.json $out/failures.document.json
        for view in list document "show sound"; do
          label=''${view%% *}
          run failures.$label 1 registry $view --root ${fixtures}/failures
          [ ! -s $out/failures.$label ]
          diff -u ${references}/failures/findings.txt <(grep -v '^shikumi: ' $out/failures.$label.stderr)
        done
        echo "failures: refused, with the tracked findings, by every view"

        # a manifest that doesn't evaluate leaves no project to read, and is the one finding.
        mkdir -p unparsed
        printf '{\n  projects = { a = "a" }\n}\n' > unparsed/shikumi.nix
        run unparsed.findings.txt 1 registry check --root unparsed
        grep -q '^error\[syntax\]: ' $out/unparsed.findings.txt
        grep -q -- '--> shikumi.nix:2:24$' $out/unparsed.findings.txt
        [ "$(grep -c '^error' $out/unparsed.findings.txt)" -eq 1 ]
        echo "unparsed manifest: one finding, at the manifest"
      '';

  # identical across worktrees, and nothing written into the checkout. the basic fixture becomes a
  # Git repository and a second worktree of the same commit, at paths that differ in every
  # component, depth and name included. the document comes out byte for byte the same from both,
  # globally and focused, and neither worktree's files change. a planted declaration that writes
  # only its worktree's directory name into the document has to make the two differ.
  registry-worktrees =
    pkgs.runCommand "registry-worktrees"
      {
        nativeBuildInputs = [
          shikumi
          pkgs.nixVersions.nix_2_35
          pkgs.git
          pkgs.diffutils
        ];
      }
      ''
        ${prelude}
        mkdir -p $out
        git() { command git -c user.name=shikumi -c user.email=shikumi@invalid -c init.defaultBranch=main "$@"; }

        # two worktrees of one commit of the given fixture, under $1 and $2.
        worktrees() {
          mkdir -p "$(dirname "$2")" "$(dirname "$3")"
          cp -r "$1" "$2"
          chmod -R u+w "$2"
          git -C "$2" init -q
          git -C "$2" add -A
          git -C "$2" commit -q -m fixture
          git -C "$2" worktree add -q "$3"
        }

        # every path, type, mode and content in a directory, Git's own files included.
        snapshot() {
          (cd "$1" && find . -printf '%p %y %m %l\n' | sort && find . -type f -print0 | sort -z | xargs -0 sha256sum)
        }

        first=$TMPDIR/a/repo
        second=$TMPDIR/second-place/deeper/checkout-two
        worktrees ${fixtures}/basic "$first" "$second"
        snapshot "$first" > $out/first-before
        snapshot "$second" > $out/second-before

        for focus in "" core docs; do
          label=''${focus:-global}
          args=(registry document)
          [ -n "$focus" ] && args+=(--focus "$focus")
          (cd "$first" && shikumi "''${args[@]}") > $out/$label-first.json
          (cd "$second" && shikumi "''${args[@]}") > $out/$label-second.json
          cmp $out/$label-first.json $out/$label-second.json
          echo "$label: identical from both worktrees"
        done

        snapshot "$first" > $out/first-after
        snapshot "$second" > $out/second-after
        diff $out/first-before $out/first-after
        diff $out/second-before $out/second-after
        echo "neither worktree changed"

        worktrees ${worktreeLeak} $TMPDIR/b/leak $TMPDIR/elsewhere/deeper/leak-two
        shikumi registry document --root $TMPDIR/b/leak > $out/leak-first.json
        shikumi registry document --root $TMPDIR/elsewhere/deeper/leak-two > $out/leak-second.json
        if cmp -s $out/leak-first.json $out/leak-second.json; then
          echo "the planted location leak produced identical documents" >&2
          exit 1
        fi
        echo "the planted location leak was caught:"
        diff $out/leak-first.json $out/leak-second.json || true
      '';

}
// lib.mapAttrs' (
  name: nix:
  # no builds during evaluation, on each implementation as its own check, so they build side by side
  # and a failure names its implementation. `declarations-no-builds.sh` holds what each one runs.
  lib.nameValuePair "registry-no-builds-${name}" (
    pkgs.runCommand "registry-no-builds-${name}"
      {
        nativeBuildInputs = [
          nix
          shikumi
          pkgs.diffutils
          pkgs.jq
          pkgs.util-linux
        ];
      }
      ''
        bash ${./declarations-no-builds.sh} ${layer} ${nixpkgs} ${fixtures} ${planted} \
          ${references} $out
      ''
  )
) implementations
