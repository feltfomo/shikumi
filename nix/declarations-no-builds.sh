# the `registry-no-builds-*` checks' body, run once per Nix implementation with that
# implementation's `nix-instantiate` first on PATH, beside `shikumi`.
#
# usage: declarations-no-builds.sh LAYER NIXPKGS FIXTURES PLANTED EXPECTED OUT
#
# the basic fixture has to produce EXPECTED, the tracked document every implementation writes, and
# the broken one has to be refused. every directory under PLANTED is a repository whose declaration
# reaches for a build or a program; its `refusal` file holds the word the producer's refusal has to
# name. the producer has to refuse each one, and a control evaluation of the same declaration
# without the producer's refusals, on a store this build owns, has to reach what it planted, so the
# refusal is what stopped it and not something else in the sandbox. `NIX_CONFIG` turns
# `builtins.exec` on for every run, as a caller's configuration could.
set -euo pipefail

layer=$1
nixpkgs=$2
fixtures=$3
planted=$4
expected=$5
out=$6

[ -x /bin/sh ] || {
  echo "the sandbox has no /bin/sh for the control builds" >&2
  exit 1
}
mkdir -p "$out"
export XDG_CACHE_HOME="$TMPDIR/cache"
export NIX_CONFIG="allow-unsafe-native-code-during-evaluation = true"

shikumi registry document --root "$fixtures/basic" > "$out/basic.json"
cmp "$out/basic.json" "$expected"
echo "basic: the tracked document"

if shikumi registry list --root "$fixtures/broken" > /dev/null 2> "$out/broken.stderr"; then
  echo "the broken fixture was listed" >&2
  exit 1
fi
grep -q 'the declarations are refused' "$out/broken.stderr"
echo "broken: refused"

cases=0
for repository in "$planted"/*/; do
  case=$(basename "$repository")
  named=$(cat "$repository/refusal")
  cases=$((cases + 1))
  if shikumi registry document --root "$repository" > /dev/null 2> "$out/$case.stderr"; then
    echo "the planted $case was not refused" >&2
    exit 1
  fi
  grep -q "$named" "$out/$case.stderr" || {
    echo "the planted $case failed without naming $named:" >&2
    cat "$out/$case.stderr" >&2
    exit 1
  }
  control="$TMPDIR/control-$case"
  mkdir -p "$control"
  nix-instantiate --eval --strict --json --read-write-mode \
    --store "local?store=$control/store&state=$control/state&log=$control/log" \
    --option sandbox false --option build-users-group "" --option substituters "" \
    "$layer/produce.nix" --argstr nixpkgs "$nixpkgs" --argstr root "$repository" \
    > "$out/$case.control.json" 2> "$out/$case.control.stderr" || {
    echo "the control evaluation of $case didn't reach what it planted:" >&2
    cat "$out/$case.control.stderr" >&2
    exit 1
  }
  echo "$case: refused; the control reached it"
done
[ "$cases" -gt 0 ] || {
  echo "no planted cases under $planted" >&2
  exit 1
}
