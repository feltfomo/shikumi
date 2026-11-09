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
# every directory under PLANTED is a repository whose declaration reaches for a build or a
# program; its `refusal` file holds the word the producer's refusal has to name. the producer has
# to refuse each one, and a control evaluation of the same declaration without the producer's
# refusals, on a store this build owns, has to reach what it planted, so the refusal is what
# stopped it and not something else in the sandbox. the cases run side by side, each with its own
# control store. `NIX_CONFIG` turns `builtins.exec` on for every run, as a caller's configuration
# could.
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

# one planted case: refused by the producer, reached by the control.
plant() {
  local repository=$1 case named control
  case=$(basename "$repository")
  named=$(cat "$repository/refusal")
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
