# the gate's checks on the declaration layer and the registry document. each runs in the Nix build
# sandbox, where there's no daemon: the producer evaluates from its own private store under the
# check's cache directory, and nix-unit and the control evaluations get a store of their own under
# the build's temporary directory. every fixture the producer reads is first made a Git repository
# with a lock by `tests/repository.sh`. each check takes only the files it reads, so editing a unit
# test reruns `declarations` alone.
{
  pkgs,
  # the packaged `shikumi`, which runs the declaration layer it ships.
  shikumi,
  nix-unit,
  # the nixpkgs source tree the layer runs on, and its NAR hash, which every check's lock pins.
  nixpkgs,
  nixpkgsHash,
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
  # them. registry-cli and registry-worktrees run on CppNix's.
  implementations = with pkgs; {
    cppnix-2-35 = nixVersions.nix_2_35;
    determinate-3-24 = import ./determinate.nix { inherit pkgs; };
    lix-2-95 = lixPackageSets.lix_2_95.lix;
    lix-2-96 = lixPackageSets.lix_2_96.lix;
  };

  # what every run needs: Nix and Shikumi keep their caches under the build's own directory, since
  # the sandbox has no home.
  prelude = ''
    set -euo pipefail
    export XDG_CACHE_HOME="$TMPDIR/cache"
  '';

  # the harness that makes a repository of a fixture, for checks that run the producer: `nixpkgs`
  # for the lock, the planted cases' `built.nix`, and a directory outside every repository.
  repositories = ''
    nixpkgs=${nixpkgs}
    nixpkgs_hash=${nixpkgsHash}
    fixtures=${fixtures}
    planted=${planted}
    outside=$TMPDIR/outside
    mkdir -p "$outside"
    source ${./declarations/tests/repository.sh}
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
          --argstr homeManager ${pkgs.home-manager.src} --argstr vocabulary ${vocabulary} \
          --argstr references ${references}
        touch $out
      '';

  # the reference page is generated from the vocabulary the checker checks with, and fails here
  # when the tracked page says anything else.
  declarations-reference =
    pkgs.runCommand "declarations-reference"
      {
        nativeBuildInputs = [ shikumi ] ++ (with pkgs; [ diffutils ]);
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
          implementations.cppnix-2-35
          shikumi
        ]
        ++ (with pkgs; [
          diffutils
          git
        ]);
      }
      ''
        ${prelude}
        ${repositories}
        mkdir -p $out
        for fixture in basic broken failures; do
          repository ${fixtures}/$fixture repos/$fixture
        done

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

        run basic.document.json 0 registry document --root repos/basic
        diff -u ${references}/basic/document.json $out/basic.document.json
        # the second run is a cache hit, which reads the document the first one kept.
        run basic.document-hit.json 0 registry document --root repos/basic
        diff -u ${references}/basic/document.json $out/basic.document-hit.json
        echo "basic: accepted, with the tracked document on stdout, from a miss and from a hit"

        # the check view prints what it was asked for, the findings, on stdout.
        run broken.findings.txt 1 registry check --root repos/broken
        diff -u ${references}/broken/findings.txt $out/broken.findings.txt
        grep -q 'the declarations are refused, with' $out/broken.findings.txt.stderr

        # every other view renders the findings on stderr, prints nothing, and fails.
        run broken.list 1 registry list --root repos/broken
        [ ! -s $out/broken.list ]
        diff -u ${references}/broken/findings.txt <(grep -v '^shikumi: ' $out/broken.list.stderr)
        grep -q '^shikumi: the declarations are refused, with' $out/broken.list.stderr
        echo "broken: refused, with the tracked findings where each view puts them"

        # declarations that fail to evaluate are findings like any other, each at the user's line,
        # beside the findings of the declarations that evaluated.
        run failures.findings.txt 1 registry check --root repos/failures
        diff -u ${references}/failures/findings.txt $out/failures.findings.txt
        run failures.document.json 1 registry check --format json --root repos/failures
        diff -u ${references}/failures/document.json $out/failures.document.json
        for view in list document "show sound"; do
          label=''${view%% *}
          run failures.$label 1 registry $view --root repos/failures
          [ ! -s $out/failures.$label ]
          diff -u ${references}/failures/findings.txt <(grep -v '^shikumi: ' $out/failures.$label.stderr)
        done
        echo "failures: refused, with the tracked findings, by every view"

        # a manifest that doesn't evaluate leaves no project to read, and is the one finding.
        mkdir -p unparsed-source
        printf '{\n  projects = { a = "a" }\n}\n' > unparsed-source/shikumi.nix
        repository unparsed-source unparsed
        run unparsed.findings.txt 1 registry check --root unparsed
        grep -q '^error\[syntax\]: ' $out/unparsed.findings.txt
        grep -q -- '--> shikumi.nix:2:24$' $out/unparsed.findings.txt
        [ "$(grep -c '^error' $out/unparsed.findings.txt)" -eq 1 ]
        echo "unparsed manifest: one finding, at the manifest"

        # a repository without a lock, and one whose lock is broken, are refused before anything is
        # evaluated, saying what's wrong with the lock.
        cp -r repos/basic unlocked
        chmod -R u+w unlocked
        git -C unlocked rm -q -r shikumi.lock
        run unlocked.document.json 1 registry document --root unlocked
        grep -q "^shikumi: the repository's lock is refused: .* has no lock" $out/unlocked.document.json.stderr
        cp -r repos/basic broken-lock
        chmod -R u+w broken-lock
        echo 'not a lock line' >> broken-lock/shikumi.lock/toolchain/nixpkgs.jsonl
        git -C broken-lock add shikumi.lock
        run broken-lock.document.json 1 registry document --root broken-lock
        grep -q "^shikumi: the repository's lock is refused: shikumi.lock/toolchain/nixpkgs.jsonl:2: " \
          $out/broken-lock.document.json.stderr
        echo "no lock, or a broken one: refused, naming the lock"
      '';

  # identical across worktrees, and nothing written into the checkout. the basic fixture becomes a
  # Git repository and a second worktree of the same commit, at paths that differ in every
  # component, depth and name included, each with an untracked file of its own. the document comes
  # out byte for byte the same from both, globally and focused, and neither worktree's files change.
  #
  # the planted leak's declarations write the name of the directory they're evaluated from, and a
  # listing of the repository's directory, into the document. the producer evaluates from the store
  # copy of the keyed files, whose listing holds only keyed files and whose name is its content's,
  # which the checker refuses as the repository's location; so both worktrees give the same
  # findings, the location's alone. the control evaluates the same declarations from each checkout
  # itself, without the copy, and has to give the two different answers, so the plant does reach
  # the location and the listing.
  registry-worktrees =
    pkgs.runCommand "registry-worktrees"
      {
        nativeBuildInputs = [
          implementations.cppnix-2-35
          shikumi
        ]
        ++ (with pkgs; [
          diffutils
          git
          jq
        ]);
      }
      ''
        ${prelude}
        ${repositories}
        mkdir -p $out

        # two worktrees of one commit of the given fixture, under $2 and $3, each with an untracked
        # file named for it in every directory a project or the catalog could list.
        worktrees() {
          repository "$1" "$2"
          mkdir -p "$(dirname "$3")"
          repository_git "$2" worktree add -q "$3"
          for directory in $(cd "$2" && find . -name '*.nix' -printf '%h\n' | sort -u); do
            echo '"untracked"' > "$2/$directory/untracked-first.nix"
            echo '"untracked"' > "$3/$directory/untracked-second.nix"
          done
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

        leak=$TMPDIR/b/leak
        elsewhere=$TMPDIR/elsewhere/deeper/leak-two
        worktrees ${worktreeLeak} "$leak" "$elsewhere"
        shikumi registry check --format json --root "$leak" > $out/leak-first.json || true
        shikumi registry check --format json --root "$elsewhere" > $out/leak-second.json || true
        cmp $out/leak-first.json $out/leak-second.json || {
          echo "the planted leak reached the document: the two worktrees' documents differ" >&2
          exit 1
        }
        codes=$(jq -c '[.diagnostics[] | [.project, .code]]' $out/leak-first.json)
        [ "$codes" = '[["app","location"]]' ] || {
          echo "the planted leak came back as $codes, not the app's location alone" >&2
          exit 1
        }
        echo "the planted leak: the same findings from both worktrees, the copy's name refused as the location"
        control() {
          nix-instantiate --eval --strict --json --store dummy:// ${layer}/produce.nix \
            --argstr nixpkgs ${nixpkgs} --arg root "$1" \
            --argstr request '{"evaluate":[{"name":"app","file":"app/project.nix"},{"name":"listing","file":"listing/project.nix"}]}'
        }
        control "$leak" > $out/leak-first.control.json
        control "$elsewhere" > $out/leak-second.control.json
        if cmp -s $out/leak-first.control.json $out/leak-second.control.json; then
          echo "the control gave both worktrees the same answer, so the plant reached nothing" >&2
          exit 1
        fi
        echo "the control, from each checkout itself, reached the location and the listing:"
        diff $out/leak-first.control.json $out/leak-second.control.json || true
      '';

  # two pins of different toolchain sources, made on two branches with `shikumi lock pin`, merge
  # without a conflict, since each lands in its own shard. the same pin moved on both branches is
  # the control: it conflicts, so the layout is what kept the first merge clean.
  lock-merge =
    pkgs.runCommand "lock-merge"
      {
        nativeBuildInputs = [ shikumi ] ++ (with pkgs; [ git ]);
      }
      ''
        ${prelude}
        source ${./declarations/tests/repository.sh}
        mkdir -p $out repo
        cd repo
        git() { repository_git . "$@"; }
        # a well-formed hash per pin; nothing is fetched, so it needn't be any tree's.
        hash() { printf 'sha256-%sAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=' "$1"; }
        pin() {
          shikumi lock pin "$1" "https://example.invalid/$1-$2.tar.gz" --hash "$(hash "$3")" >> $out/pins
          git add shikumi.lock
          git commit -q -m "pin $1 $2"
        }

        git init -q
        pin nixpkgs base B
        git branch other
        pin nixpkgs moved C
        git checkout -q other
        pin nix-unit added D
        git checkout -q main
        git merge -q --no-edit other 2>&1 | tee $out/merge-different-shards
        git ls-files -u | tee $out/unmerged-different-shards
        [ -z "$(git ls-files -u)" ]
        grep -q nixpkgs-moved shikumi.lock/toolchain/nixpkgs.jsonl
        grep -q nix-unit-added shikumi.lock/toolchain/nix-unit.jsonl
        echo "pins of different sources on two branches: merged cleanly"

        git checkout -q -b again HEAD~2
        pin nixpkgs elsewhere E
        git checkout -q main
        if git merge -q --no-edit again > $out/merge-same-shard 2>&1; then
          echo "the same pin moved on both branches merged without a conflict" >&2
          exit 1
        fi
        git ls-files -u | tee $out/unmerged-same-shard
        echo "the control: the same pin moved on both branches conflicts in its one shard"
      '';
}
// lib.mapAttrs' (
  name: implementation:
  # no builds during evaluation, on each implementation as its own check, so they build side by side
  # and a failure names its implementation. `declarations-no-builds.sh` holds what each one runs.
  lib.nameValuePair "registry-no-builds-${name}" (
    pkgs.runCommand "registry-no-builds-${name}"
      {
        nativeBuildInputs = [
          implementation
          shikumi
        ]
        ++ (with pkgs; [
          util-linux
          diffutils
          git
          jq
        ]);
      }
      ''
        bash ${./declarations-no-builds.sh} ${layer} ${nixpkgs} ${fixtures} ${planted} \
          ${references} ${./declarations/tests/repository.sh} ${nixpkgsHash} $out
      ''
  )
) implementations
