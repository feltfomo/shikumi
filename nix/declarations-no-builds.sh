# the `registry-no-builds-*` checks' body, run once per Nix implementation with that
# implementation's `nix-instantiate` first on PATH, beside `shikumi`.
#
# usage: declarations-no-builds.sh LAYER NIXPKGS FIXTURES PLANTED REFERENCES OUT
#
# what an implementation decides is the serialized declarations, so each fixture repository goes
# through the producer once and has to come out as REFERENCES/<fixture>/declarations.json byte for
# byte, with the fixture's location written as `@root@`. the document and the rendered findings are
# Rust's work over those bytes; the registry crate's tests pin them to the same references, so
# identical declarations give identical findings on every implementation.
#
# the failures fixture's declarations mostly fail to evaluate. what each implementation prints for
# a failure is its own, so the comparison covers what Shikumi read from it, against the same
# reference: each failure's kind, whose it is, its frames in the user's files by file and line,
# innermost first, and the field it names; not the evaluator's own message, and not the columns,
# which implementations place at different points of one expression. the findings as a person reads
# them on this implementation are kept in OUT, and may name neither the declaration layer nor a
# store path. the fixture is also evaluated on one core, where its projects share batches, and each
# project is evaluated alone, focused on it, side by side; the two have to agree on every project's
# entry. a fault planted in a copy of the layer, where it serializes each project, has to come back
# as a Shikumi bug wherever it's reached, and the user's own failures as theirs.
#
# every directory under PLANTED is a repository whose declaration, or catalog, reaches for a build
# or a program; its `refusal` file holds the word the producer's refusal has to name, and its
# `request` file, where it has one, the evaluation the control asks for in place of the `app`
# project's declaration, which REFERENCES' `planted-request.json` asks for, held by the registry
# crate's tests to the producer's own request. the producer has to refuse each one, and a control
# evaluation of the same request without the producer's refusals, on a store this build owns, has
# to reach what it planted, so the refusal is what stopped it and not something else in the
# sandbox. the cases run side by side, each with its own control store. `NIX_CONFIG` turns
# `builtins.exec` on for every run, as a caller's configuration could.
#
# `jq` reads the JSON and `taskset` limits a run to one core.
set -euo pipefail

layer=$1
nixpkgs=$2
fixtures=$3
planted=$4
references=$5
out=$6

[ -x /bin/sh ] || {
  echo "the sandbox has no /bin/sh for the control builds" >&2
  exit 1
}
mkdir -p "$out"
export XDG_CACHE_HOME="$TMPDIR/cache"
export NIX_CONFIG="allow-unsafe-native-code-during-evaluation = true"

for fixture in basic broken; do
  root=$fixtures/$fixture
  shikumi registry declarations --root "$root" > "$out/$fixture.declarations.raw" \
    2> "$out/$fixture.declarations.stderr" || {
    echo "the producer couldn't serialize the $fixture fixture:" >&2
    cat "$out/$fixture.declarations.stderr" >&2
    exit 1
  }
  serialized=$(cat "$out/$fixture.declarations.raw")
  printf '%s\n' "${serialized//"$root"/@root@}" > "$out/$fixture.declarations.json"
  diff -u "$references/$fixture/declarations.json" "$out/$fixture.declarations.json" || {
    echo "$fixture: the serialized declarations differ from the tracked reference" >&2
    exit 1
  }
  echo "$fixture: the tracked serialized declarations"
done

# what's compared of a failure: everything Shikumi read from it but the evaluator's own words and
# the columns of its positions, which implementations place at different points of one expression.
classified='(.. | objects | select(has("failure")) | .failure)
  |= (del(.message?) | (.. | objects | select(has("line") and has("column"))) |= del(.column))'

failures=$fixtures/failures
shikumi registry declarations --root "$failures" > "$out/failures.declarations.raw" \
  2> "$out/failures.declarations.stderr" || {
  echo "the producer couldn't read the failures fixture:" >&2
  cat "$out/failures.declarations.stderr" >&2
  exit 1
}
jq -S "$classified" "$out/failures.declarations.raw" > "$out/failures.classified.json"
jq -S "$classified" "$references/failures/declarations.json" > "$out/failures.reference.json"
diff -u "$out/failures.reference.json" "$out/failures.classified.json" || {
  echo "failures: the classified failures differ from the tracked reference" >&2
  exit 1
}
echo "failures: every failure classified as the tracked reference has it"

if shikumi registry check --root "$failures" > "$out/failures.findings.txt" \
  2> "$out/failures.findings.stderr"; then
  echo "failures: the findings were accepted" >&2
  exit 1
fi
if grep -n -e "$layer" -e /nix/store/ "$out/failures.findings.txt" >&2; then
  echo "failures: the findings above name Shikumi's own files or a store path" >&2
  exit 1
fi
echo "failures: the findings, as this implementation renders them, name no Shikumi or store path"

taskset -c 0 shikumi registry declarations --root "$failures" > "$out/failures.one-core.json"
pids=()
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
  exit 1
}
echo "failures: every project's entry on one core is what it is evaluated alone"

# every project's declaration is serialized by one line of `compose.nix`, and the planted fault
# fails there once the declaration is forced whole, so it's reached by every project whose own code
# evaluates, and by nothing else.
cp -r "$layer" "$TMPDIR/planted-layer"
chmod -R u+w "$TMPDIR/planted-layer"
sed -i 's/      value = serialized.value (if factory then declaration.declaration else declaration);/      value = builtins.deepSeq (serialized.value (if factory then declaration.declaration else declaration)) (null ++ null);/' "$TMPDIR/planted-layer/compose.nix"
grep -q 'else declaration)) (null ++ null);' "$TMPDIR/planted-layer/compose.nix" || {
  echo "the fault wasn't planted: compose.nix no longer has the line it replaces" >&2
  exit 1
}
# the package sets the layer it ships, so a copy of its wrapper points at the planted one instead.
sed "s|$layer|$TMPDIR/planted-layer|g" "$(command -v shikumi)" > "$TMPDIR/shikumi-planted"
chmod +x "$TMPDIR/shikumi-planted"
grep -q "$TMPDIR/planted-layer" "$TMPDIR/shikumi-planted" || {
  echo "the shikumi wrapper doesn't name the layer it ships" >&2
  exit 1
}
if "$TMPDIR/shikumi-planted" registry check --format json --root "$failures" \
  > "$out/planted-bug.json" 2> "$out/planted-bug.stderr"; then
  echo "the planted fault in Shikumi's Nix was accepted" >&2
  exit 1
fi
# the projects whose declarations evaluate reach the broken line; the ones that fail first, in the
# user's code, don't, and the catalog, which no project's line serializes, keeps its own failure.
codes=$(jq -c '[.diagnostics[] | [.project, .code]]' "$out/planted-bug.json")
expected='[[null,"thrown"],["assertion","assertion"],["call-type","type-error"],["coerce","type-error"],["consumer","shikumi-bug"],["library","thrown"],["missing-summary","shikumi-bug"],["sound","shikumi-bug"],["syntax","syntax"],["thrown","thrown"],["undefined","undefined-variable"],["uneven","thrown"],["utf8","shikumi-bug"]]'
[ "$codes" = "$expected" ] || {
  echo "the planted fault came back as $codes, not $expected" >&2
  exit 1
}
echo "planted fault: a Shikumi bug wherever it's reached, the user's own failures still theirs"

# one planted case: refused by the producer, reached by the control.
plant() {
  local repository=$1 case named control request
  case=$(basename "$repository")
  named=$(cat "$repository/refusal")
  request=$(cat "$references/planted-request.json")
  if [ -f "$repository/request" ]; then
    request=$(cat "$repository/request")
  fi
  if shikumi registry document --root "$repository" > /dev/null 2> "$out/$case.stderr"; then
    echo "the planted $case was not refused"
    return 1
  fi
  grep -q "$named" "$out/$case.stderr" || {
    echo "the planted $case failed without naming $named:"
    cat "$out/$case.stderr"
    return 1
  }
  control="$TMPDIR/control-$case"
  mkdir -p "$control"
  nix-instantiate --eval --strict --json --read-write-mode \
    --store "local?store=$control/store&state=$control/state&log=$control/log" \
    --option sandbox false --option build-users-group "" --option substituters "" \
    "$layer/produce.nix" --argstr nixpkgs "$nixpkgs" --argstr root "$repository" \
    --argstr request "$request" \
    > "$out/$case.control.json" 2> "$out/$case.control.stderr" || {
    echo "the control evaluation of $case didn't reach what it planted:"
    cat "$out/$case.control.stderr"
    return 1
  }
  echo "$case: refused; the control reached it"
}

cases=()
pids=()
for repository in "$planted"/*/; do
  case=$(basename "$repository")
  cases+=("$case")
  plant "$repository" > "$out/$case.result" 2>&1 &
  pids+=($!)
done
[ "${#cases[@]}" -gt 0 ] || {
  echo "no planted cases under $planted" >&2
  exit 1
}
# every case is waited for and reported, in name order, before a failure fails the check.
failed=0
for index in "${!cases[@]}"; do
  wait "${pids[$index]}" || failed=1
  cat "$out/${cases[$index]}.result"
done
exit "$failed"
