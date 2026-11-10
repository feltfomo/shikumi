# the `registry-no-builds-*` checks' body, run once per Nix implementation with that
# implementation's `nix-instantiate` first on PATH, beside `shikumi` and `git`.
#
# usage: declarations-no-builds.sh LAYER NIXPKGS FIXTURES PLANTED REFERENCES REPOSITORY NIXPKGS_HASH OUT [STEP...]
#
# the check is a set of steps that share nothing but the fixture repositories they read, so they run
# side by side, each with its output in OUT/<step>.log, printed in the order below once all have
# ended. given STEPs, only those run, which is how one is run on its own while working. every
# fixture and planted case is first made a Git repository with a lock by REPOSITORY's `repository`,
# since the producer reads only what Git tracks, under a lock.
#
# `fixtures`: what an implementation decides is the serialized declarations, so each fixture
# repository goes through the producer once and has to come out as
# REFERENCES/<fixture>/declarations.json byte for byte, with the location of the store copy it was
# evaluated from written as `@root@`. the document and the rendered findings are Rust's work over
# those bytes; the registry crate's tests pin them to the same references, so identical
# declarations give identical findings on every implementation.
#
# `failures`: the failures fixture's declarations mostly fail to evaluate. what each implementation
# prints for a failure is its own, so the comparison covers what Shikumi read from it, against the
# same reference: each failure's kind, whose it is, its frames in the user's files by file and line,
# innermost first, and the field it names; not the evaluator's own message, and not the columns,
# which implementations place at different points of one expression. the findings as a person reads
# them on this implementation are kept in OUT, and may name neither the declaration layer nor a
# store path. the fixture is also evaluated on one core, where its projects share batches, and each
# project is evaluated alone, focused on it, side by side; the two have to agree on every project's
# entry. a fault planted in a copy of the layer, where it serializes each project, has to come back
# as a Shikumi bug wherever it's reached, and the user's own failures as theirs.
#
# `planted`: every other directory under PLANTED is a repository whose declaration, or catalog,
# reaches for something the producer refuses: a build or its output, a program, or a read pure
# evaluation from the keyed copy doesn't allow. its `refusal` file holds the text a finding's
# `error` line has to hold, so neither a frame of the trace nor a line of source the finding shows
# can stand in for the refusal; its `request` file, where it has one, the evaluation the control
# asks for in place of the `app` project's declaration, which REFERENCES' `planted-request.json`
# asks for, held by the registry crate's tests to the producer's own request; its `control` file,
# where it has one, more arguments for the control, a line each; its `warm` file, where it has one,
# runs the control once before the run that counts; its `reached` file, where it has one, the lines
# one of which the control's answer or error has to hold, a line for each way an implementation
# reaches it; and its `outside-as` file, where it has one, how its `outside/` directory is put
# outside every repository, at `$outside/<case>`: as a `directory`, or as a `git` repository
# committed at a fixed date, so a locked reference to it names the same tree everywhere. the
# producer has to refuse each one, and a control evaluation of the same request without the
# producer's refusals, impure, on a store this build owns and through a copy of the layer that
# withholds no builtin, has to reach what it planted, so the refusal is what stopped it and not
# something else in the sandbox. the cases run side by side, each with its own control store.
# `NIX_CONFIG` turns `builtins.exec` on for every run, as a caller's configuration could.
#
# a planted case with a `caches` file reads where the evaluator reads its copy instead: for each view
# the file names, a line each, it's run under this check's cache directory and under a second one
# of another length. the two runs have to print the same, or both be refused with the same text,
# the document's refusal, where the case has a `refusal` file, a finding whose `error` line holds
# its text. an evaluator that gives a declaration the copy where it lies under the cache directory
# is the one that would print two.
#
# a planted case with a `context` file is the toolchain context's instead: the producer resolves its
# catalog's package to a store path without building it, and a control build of the same entry
# writes exactly that path.
#
# `window` holds the private store's copies against a collection, in a cache of its own; it says
# what it holds where it's defined.
#
# `jq` reads the JSON and `taskset` limits a run to one core.
set -euo pipefail

layer=$1
nixpkgs=$2
fixtures=$3
planted=$4
references=$5
helpers=$6
nixpkgs_hash=$7
out=$8
shift 8
steps=("$@")
[ "${#steps[@]}" -gt 0 ] || steps=(fixtures window failures planted)

[ -x /bin/sh ] || {
  echo "the sandbox has no /bin/sh for the control builds" >&2
  exit 1
}
mkdir -p "$out"
export XDG_CACHE_HOME="$TMPDIR/cache"
# the second cache directory the `caches` cases run under, of another length than the first.
second_cache=$TMPDIR/a-second-cache-directory
export NIX_CONFIG="allow-unsafe-native-code-during-evaluation = true"

outside=$TMPDIR/outside
mkdir -p "$outside"
echo '"Imported from outside the commit"' > "$outside/value.nix"
echo '"Read through the search path"' > "$outside/channel.nix"
# shellcheck source=nix/declarations/tests/repository.sh
source "$helpers"

# the layer the controls evaluate through: the shipped one with nothing withheld from its scope.
control_layer=$TMPDIR/control-layer
cp -r "$layer" "$control_layer"
chmod -R u+w "$control_layer"
jq '.withheld = []' "$layer/layer.json" > "$control_layer/layer.json"

# the store copies the producer evaluates from, wherever their location is written out, by the name
# the layer's table gives them.
keyed=$(jq -r .storeNames.keyed "$layer/layer.json")
copies="s#(/[^/\" ]+)*/nix/store/[0-9a-z]{32}-$keyed#@root@#g"

real_bin=$(dirname "$(command -v nix-instantiate)")
# a wrapped evaluator runs with its environment cleared, so its script names what it runs in full.
jq_bin=$(command -v jq)
grep_bin=$(command -v grep)
sed_bin=$(command -v sed)
flock_bin=$(command -v flock)

for fixture in basic broken failures; do
  repository "$fixtures/$fixture" "$TMPDIR/repos/$fixture"
done

# a directory of this implementation's programs with `nix-instantiate` replaced by the script on
# stdin, which ends by running the real one.
wrapped() {
  local bin=$1 program
  mkdir -p "$bin"
  for program in "$real_bin"/*; do
    ln -s "$program" "$bin/$(basename "$program")"
  done
  rm "$bin/nix-instantiate"
  cat > "$bin/nix-instantiate"
  chmod +x "$bin/nix-instantiate"
}

# the basic fixture's serialized declarations from the producer as run by `$@`, held to the tracked
# reference; `$1` names the run's files in OUT.
reference_run() {
  local name=$1
  shift
  "$@" shikumi registry declarations --root "$TMPDIR/repos/basic" > "$out/$name.raw" \
    2> "$out/$name.stderr" || {
    echo "$name: the producer failed:" >&2
    cat "$out/$name.stderr" >&2
    return 1
  }
  sed -E "$copies" "$out/$name.raw" | diff -u "$references/basic/declarations.json" - || {
    echo "$name: the serialized declarations differ from the tracked reference" >&2
    return 1
  }
}

step_fixtures() {
  local fixture root
  for fixture in basic broken; do
    root=$TMPDIR/repos/$fixture
    shikumi registry declarations --root "$root" > "$out/$fixture.declarations.raw" \
      2> "$out/$fixture.declarations.stderr" || {
      echo "the producer couldn't serialize the $fixture fixture:" >&2
      cat "$out/$fixture.declarations.stderr" >&2
      return 1
    }
    sed -E "$copies" "$out/$fixture.declarations.raw" > "$out/$fixture.declarations.json"
    diff -u "$references/$fixture/declarations.json" "$out/$fixture.declarations.json" || {
      echo "$fixture: the serialized declarations differ from the tracked reference" >&2
      return 1
    }
    echo "$fixture: the tracked serialized declarations"
  done
}

# a collection in the window between a run's roots and its copies landing: there it reads a root
# that names a path not yet valid and skips it (cppnix 2.35.3 src/libstore/gc.cc:217-228,
# determinate 3.24.2 gc.cc:296-305, lix 2.95.4 and 2.96.0 lix/libstore/gc.cc:263-273), and once
# the copying evaluation has exited, takes the copy. an evaluator whose copying run, each time a copy lands, tries to take the store's
# collection lock alone at once, as a collection does, and where it can, deletes the keyed copies
# it just added as that collection would, liveness ignored. the run has to keep every collection
# out of the window, so no attempt gets the lock and the basic fixture comes out as its reference
# has it.
step_window() {
  local attempts=$TMPDIR/window-attempts
  wrapped "$TMPDIR/window-bin" <<WRAPPER
#!/bin/sh
store=
copying=
previous=
for argument in "\$@"; do
  [ "\$previous" = --eval-store ] && store=\$argument
  [ "\$argument" = --read-write-mode ] && copying=1
  previous=\$argument
done
printed=\$($real_bin/nix-instantiate "\$@") || exit \$?
printf '%s\n' "\$printed"
if [ -n "\$copying" ]; then
  root=\$(printf '%s' "\$store" | $sed_bin -n 's/^local?root=\([^&]*\).*/\1/p')
  state=\$(printf '%s' "\$store" | $sed_bin -n 's/.*&state=\([^&]*\).*/\1/p')
  [ -n "\$state" ] || state=\$root/nix/var/nix
  if $flock_bin -xn "\$state/gc.lock" true; then
    echo collected >> "$attempts"
    for path in \$(printf '%s' "\$printed" | $jq_bin -r '.[]' | $grep_bin -- '-$keyed\$'); do
      $real_bin/nix-store --store "\$store" --delete --ignore-liveness "\$path" > /dev/null 2>&1
    done
  else
    echo held >> "$attempts"
  fi
fi
WRAPPER
  reference_run window env XDG_CACHE_HOME="$TMPDIR/window-cache" PATH="$TMPDIR/window-bin:$PATH" || {
    cat "$attempts" >&2
    return 1
  }
  [ -s "$attempts" ] || {
    echo "the evaluator that tries the window never copied anything" >&2
    return 1
  }
  if "$grep_bin" -q collected "$attempts"; then
    echo "a collection could start between a run's roots and its copies landing" >&2
    return 1
  fi
  echo "no collection between a run's roots and its copies landing: the tracked serialized declarations"
}

# what's compared of a failure: everything Shikumi read from it but the evaluator's own words and
# the columns of its positions, which implementations place at different points of one expression.
classified='(.. | objects | select(has("failure")) | .failure)
  |= (del(.message?) | (.. | objects | select(has("line") and has("column"))) |= del(.column))'

step_failures() {
  local failures=$TMPDIR/repos/failures project differing codes expected pid
  local pids=()
  shikumi registry declarations --root "$failures" > "$out/failures.declarations.raw" \
    2> "$out/failures.declarations.stderr" || {
    echo "the producer couldn't read the failures fixture:" >&2
    cat "$out/failures.declarations.stderr" >&2
    return 1
  }
  jq -S "$classified" "$out/failures.declarations.raw" > "$out/failures.classified.json"
  jq -S "$classified" "$references/failures/declarations.json" > "$out/failures.reference.json"
  diff -u "$out/failures.reference.json" "$out/failures.classified.json" || {
    echo "failures: the classified failures differ from the tracked reference" >&2
    return 1
  }
  echo "failures: every failure classified as the tracked reference has it"

  if shikumi registry check --root "$failures" > "$out/failures.findings.txt" \
    2> "$out/failures.findings.stderr"; then
    echo "failures: the findings were accepted" >&2
    return 1
  fi
  if grep -n -e "$layer" -e /nix/store/ "$out/failures.findings.txt" >&2; then
    echo "failures: the findings above name Shikumi's own files or a store path" >&2
    return 1
  fi
  echo "failures: the findings, as this implementation renders them, name no Shikumi or store path"

  taskset -c 0 shikumi registry declarations --root "$failures" > "$out/failures.one-core.json"
  for project in $(jq -r '.projects | keys[]' "$out/failures.one-core.json"); do
    shikumi registry declarations --root "$failures" --focus "$project" \
      > "$out/alone.$project.json" 2> "$out/alone.$project.stderr" &
    pids+=($!)
  done
  for pid in "${pids[@]}"; do
    wait "$pid"
  done
  # every entry of every focused composition, against the same project's entry on one core.
  differing=$(jq -rn --slurpfile together "$out/failures.one-core.json" '
    inputs | input_filename as $file | .projects | to_entries[]
    | select(.value != $together[0].projects[.key]) | "\(.key), focused in \($file)"
  ' "$out"/alone.*.json)
  [ -z "$differing" ] || {
    echo "failures: evaluated beside the others, these differ from evaluated alone:" >&2
    echo "$differing" >&2
    return 1
  }
  echo "failures: every project's entry on one core is what it is evaluated alone"

  # every project's declaration is serialized by one line of `compose.nix`, and the planted fault
  # fails there once the declaration is forced whole, so it's reached by every project whose own
  # code evaluates, and by nothing else.
  cp -r "$layer" "$TMPDIR/planted-layer"
  chmod -R u+w "$TMPDIR/planted-layer"
  sed -i 's/      value = serialized.value (if factory then declaration.declaration else declaration);/      value = builtins.deepSeq (serialized.value (if factory then declaration.declaration else declaration)) (null ++ null);/' "$TMPDIR/planted-layer/compose.nix"
  grep -q 'else declaration)) (null ++ null);' "$TMPDIR/planted-layer/compose.nix" || {
    echo "the fault wasn't planted: compose.nix no longer has the line it replaces" >&2
    return 1
  }
  # the package sets the layer it ships, so a copy of its wrapper points at the planted one instead.
  sed "s|$layer|$TMPDIR/planted-layer|g" "$(command -v shikumi)" > "$TMPDIR/shikumi-planted"
  chmod +x "$TMPDIR/shikumi-planted"
  grep -q "$TMPDIR/planted-layer" "$TMPDIR/shikumi-planted" || {
    echo "the shikumi wrapper doesn't name the layer it ships" >&2
    return 1
  }
  if "$TMPDIR/shikumi-planted" registry check --format json --root "$failures" \
    > "$out/planted-bug.json" 2> "$out/planted-bug.stderr"; then
    echo "the planted fault in Shikumi's Nix was accepted" >&2
    return 1
  fi
  # the projects whose declarations evaluate reach the broken line; the ones that fail first, in
  # the user's code, don't, and the catalog, which no project's line serializes, keeps its own
  # failure.
  codes=$(jq -c '[.diagnostics[] | [.project, .code]]' "$out/planted-bug.json")
  expected='[[null,"thrown"],["assertion","assertion"],["call-type","type-error"],["coerce","type-error"],["consumer","shikumi-bug"],["library","thrown"],["missing-summary","shikumi-bug"],["sound","shikumi-bug"],["syntax","syntax"],["thrown","thrown"],["undefined","undefined-variable"],["uneven","thrown"],["utf8","shikumi-bug"]]'
  [ "$codes" = "$expected" ] || {
    echo "the planted fault came back as $codes, not $expected" >&2
    return 1
  }
  echo "planted fault: a Shikumi bug wherever it's reached, the user's own failures still theirs"
}

# the control: the request evaluated without the producer's refusals, impure, in read-write mode,
# on a store of its own, with the case's extra arguments. a case with a `warm` file is evaluated
# once first: Determinate Nix writes a path `builtins.toFile` makes on a thread of its own, and
# only a reader through the path's context waits for it (determinate 3.24.2
# src/libexpr/primops.cc:2840-2845 and src/libstore/async-path-writer.cc:52-94), so a plant that
# strips the context reads it only once an earlier run has written it. a case with a `victim` repository finds that repository's keyed copy in the
# control's store, as the producer's store holds it after evaluating the victim.
control() {
  local repository=$1 case=$2 request=$3 store=$TMPDIR/control-$2 argument run
  local extra=()
  if [ -f "$planted/$case/control" ]; then
    while read -r argument; do
      extra+=("${argument//@outside@/$outside}")
    done < "$planted/$case/control"
  fi
  mkdir -p "$store"
  local control_store="local?store=$store/store&state=$store/state&log=$store/log"
  if [ -d "$planted/$case/victim" ]; then
    nix-store --store "$control_store" --add "$(staged "$TMPDIR/planted/$case-victim" "$store")" \
      > /dev/null
  fi
  for run in warm counted; do
    if [ "$run" = warm ] && [ ! -f "$planted/$case/warm" ]; then
      continue
    fi
    # a cache directory of its own too: controls run side by side, and an evaluator writes there,
    # its flake registry among it, without expecting another beside it.
    XDG_CACHE_HOME=$store/cache nix-instantiate --eval --strict --json --read-write-mode \
      --store "$control_store" \
      --option sandbox false --option build-users-group "" --option substituters "" \
      "${extra[@]}" "$control_layer/produce.nix" --argstr nixpkgs "$nixpkgs" --arg root "$repository" \
      --argstr request "$request" > "$out/$case.control.$run.json" \
      2> "$out/$case.control.$run.stderr" || [ "$run" = warm ] || return 1
  done
}

# a repository's keyed copy as the producer stages it, its manifest, its `.nix` files and the root
# marker, under the copy's name, in a directory of DESTINATION's; prints the copy's path.
staged() {
  local repository=$1 stage=$2/staged/$keyed marker file
  marker=$(jq -r .storeNames.rootMarker "$layer/layer.json")
  mkdir -p "$stage"
  (cd "$repository" && git ls-files '*.nix') | while read -r file; do
    mkdir -p "$stage/$(dirname "$file")"
    cp "$repository/$file" "$stage/$file"
  done
  printf './.\n' > "$stage/$marker"
  printf '%s' "$stage"
}

# one planted case: refused by the producer, reached by the control. a case with a `reached` file is
# reached when the control's answer or error holds one of its lines, since an implementation can
# stop at what was planted rather than go through it; any other is reached when the control
# evaluates.
plant() {
  local case=$1 repository=$TMPDIR/planted/$1 named request reached=0
  named=$(cat "$planted/$case/refusal")
  request=$(cat "$references/planted-request.json")
  if [ -f "$planted/$case/request" ]; then
    request=$(cat "$planted/$case/request")
  fi
  if [ -d "$planted/$case/victim" ]; then
    shikumi registry document --root "$TMPDIR/planted/$case-victim" > /dev/null \
      2> "$out/$case.victim.stderr" || {
      echo "the victim of $case wasn't accepted:"
      cat "$out/$case.victim.stderr"
      return 1
    }
  fi
  if shikumi registry document --root "$repository" > /dev/null 2> "$out/$case.stderr"; then
    echo "the planted $case was not refused"
    return 1
  fi
  grep '^error' "$out/$case.stderr" | grep -qF -- "$named" || {
    echo "the planted $case failed without a finding that holds $named:"
    cat "$out/$case.stderr"
    return 1
  }
  control "$repository" "$case" "$request" && reached=1
  if [ -f "$planted/$case/reached" ]; then
    cat "$out/$case.control.counted.json" "$out/$case.control.counted.stderr" \
      | grep -qF -f "$planted/$case/reached" && reached=1 || reached=0
  fi
  [ "$reached" = 1 ] || {
    echo "the control evaluation of $case didn't reach what it planted:"
    cat "$out/$case.control.counted.json" "$out/$case.control.counted.stderr"
    return 1
  }
  echo "$case: refused; the control reached it"
}

# a case read under two cache directories: the same both times, or refused both times.
caches() {
  local case=$1 repository=$TMPDIR/planted/$1 view cache named=
  if [ -f "$planted/$case/refusal" ]; then
    named=$(cat "$planted/$case/refusal")
  fi
  while read -r view; do
    local codes=()
    for cache in "$XDG_CACHE_HOME" "$second_cache"; do
      local run=$out/$case.$view.$(basename "$cache")
      XDG_CACHE_HOME=$cache shikumi registry "$view" --root "$repository" > "$run.out" \
        2> "$run.stderr" && codes+=(0) || codes+=(1)
    done
    local first=$out/$case.$view.$(basename "$XDG_CACHE_HOME")
    local second=$out/$case.$view.$(basename "$second_cache")
    if [ "${codes[*]}" = "0 0" ]; then
      cmp -s "$first.out" "$second.out" || {
        echo "$case: $view differs between two cache directories:"
        diff "$first.out" "$second.out" | head -20
        return 1
      }
    elif [ "${codes[*]}" = "1 1" ]; then
      cmp -s "$first.stderr" "$second.stderr" || {
        echo "$case: $view's refusal differs between two cache directories:"
        diff "$first.stderr" "$second.stderr" | head -20
        return 1
      }
      if [ "$view" = document ] && [ -n "$named" ]; then
        grep '^error' "$first.stderr" | grep -qF -- "$named" || {
          echo "the planted $case failed without a finding that holds $named:"
          cat "$first.stderr"
          return 1
        }
      fi
    else
      echo "$case: $view was refused under one cache directory and not the other:"
      cat "$first.stderr" "$second.stderr"
      return 1
    fi
  done < "$planted/$case/caches"
  echo "$case: the same under two cache directories, or refused as a location read"
}

# the toolchain context's case: every system's path computed and none of them built, and the path a
# control build of the same entry writes is the one the context gives.
context() {
  local case=$1 repository=$TMPDIR/planted/$1 store=$TMPDIR/control-$1 system computed built
  shikumi registry context --root "$repository" > "$out/$case.json" 2> "$out/$case.stderr" || {
    echo "the producer couldn't compute the toolchain context of $case:"
    cat "$out/$case.stderr"
    return 1
  }
  for computed in $(jq -r '.systems[].packages.planted' "$out/$case.json"); do
    local built_anywhere=0 private
    for private in "$XDG_CACHE_HOME"/shikumi/*/stores/*; do
      [ -e "$private/nix/store/${computed#/nix/store/}" ] && built_anywhere=1
    done
    if [ "$computed" = null ] || [ -e "$computed" ] || [ "$built_anywhere" = 1 ]; then
      echo "$case: the context's path $computed is missing or was built"
      return 1
    fi
  done
  system=$(nix-instantiate --eval --store dummy:// --expr builtins.currentSystem | tr -d '"')
  mkdir -p "$store"
  built=$(nix-build --no-out-link --store "local?root=$store" \
    --option sandbox false --option build-users-group "" --option substituters "" \
    --expr "(import $repository/catalog/catalog.nix { }).overrides.shikumi-planted.package { stdenv.hostPlatform.system = \"$system\"; } { }" \
    2> "$out/$case.control.stderr") || {
    echo "the control build of $case failed:"
    cat "$out/$case.control.stderr"
    return 1
  }
  computed=$(jq -r --arg system "$system" '.systems[$system].packages.planted' "$out/$case.json")
  [ "$built" = "$computed" ] || {
    echo "$case: a build writes $built, and the context gives $computed"
    return 1
  }
  echo "$case: every system's path computed, none built; a build writes the path the context gives"
}

step_planted() {
  local directory case index failed=0
  local cases=() pids=()
  for directory in "$planted"/*/; do
    case=$(basename "$directory")
    cases+=("$case")
    if [ -f "$directory/outside-as" ]; then
      cp -r "$directory/outside" "$outside/$case"
      chmod -R u+w "$outside/$case"
      if [ "$(cat "$directory/outside-as")" = git ]; then
        repository_git "$outside/$case" init -q
        repository_git "$outside/$case" add -A
        GIT_AUTHOR_DATE=2000-01-01T00:00:00Z GIT_COMMITTER_DATE=2000-01-01T00:00:00Z \
          repository_git "$outside/$case" commit -q -m outside
      fi
    fi
    repository "$directory" "$TMPDIR/planted/$case"
    if [ -d "$directory/victim" ]; then
      repository "$directory/victim" "$TMPDIR/planted/$case-victim"
    fi
    if [ -f "$directory/context" ]; then
      context "$case" > "$out/$case.result" 2>&1 &
    elif [ -f "$directory/caches" ]; then
      caches "$case" > "$out/$case.result" 2>&1 &
    else
      plant "$case" > "$out/$case.result" 2>&1 &
    fi
    pids+=($!)
  done
  [ "${#cases[@]}" -gt 0 ] || {
    echo "no planted cases under $planted" >&2
    return 1
  }

  # what this implementation's search path is with `nix-path` empty and neither pure nor restricted
  # evaluation on, kept as evidence. CppNix and Determinate Nix add their default entries, root's
  # channels among them, after the setting, for each one that exists, and Lix only the setting's
  # (cppnix 2.35.3 src/libexpr/eval.cc:341-355, determinate 3.24.2 eval.cc:379-393, lix 2.95.4
  # lix/libexpr/eval.cc:364-372, lix 2.96.0 eval.cc:361-369);
  # the sandbox has no channels, so here it's empty on all four. the `search-path` case is what
  # holds the producer to refusing every lookup whatever the search path holds.
  nix-instantiate --eval --strict --json --store dummy:// --option nix-path '' \
    --expr builtins.nixPath > "$out/search-path.default.json"
  echo "search path with nix-path empty, outside pure mode: $(cat "$out/search-path.default.json")"

  # every case is waited for and reported, in name order, before a failure fails the step.
  for index in "${!cases[@]}"; do
    wait "${pids[$index]}" || failed=1
    cat "$out/${cases[$index]}.result"
  done
  return "$failed"
}

# every step is waited for and reported, in the order given, with how long it took, before a
# failure fails the check. a step is called on its own, not in a condition, so `set -e` holds inside
# it.
pids=()
for step in "${steps[@]}"; do
  (
    started=$SECONDS
    trap 'echo "$step: $((SECONDS - started)) s, exit $?"' EXIT
    "step_$step"
  ) > "$out/$step.log" 2>&1 &
  pids+=($!)
done
failed=0
for index in "${!steps[@]}"; do
  wait "${pids[$index]}" || failed=1
  cat "$out/${steps[$index]}.log"
done
exit "$failed"
