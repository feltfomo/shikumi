# the checks' one way of making a repository from a tracked fixture or planted case: Shikumi reads
# only what Git tracks, under a lock, so each becomes a Git repository with its lock pinning the
# nixpkgs the checks run on. sourced by the checks, after setting:
#
# - `nixpkgs` and `nixpkgs_hash`: the nixpkgs source tree and its NAR hash, which every lock pins.
#   the producer finds that tree where it already is, so nothing is fetched.
# - `fixtures`: the fixtures' directory, whose `held-as` file names the extension a tracked file is
#   held under and the one every repository holds it as, for Nix the gate's formatter and linter
#   can't parse.
# - `planted`: the planted cases' directory, whose `built.nix` is copied beside every file that
#   imports `./built.nix`, and whose `shikumi.nix` is the manifest of every case without one.
# - `outside`: a directory outside every repository, which a case's symlink can point into.
#
# `@outside@` in a case's `.nix` files stands for `$outside` too, so a declaration can name a path
# there.
#
# a case can hold two files that shape its repository and are then removed from it: `links`, a
# symlink per line as `path target`, with `@outside@` standing for `$outside`; and `untracked`, a path
# per line that stays on disk but out of Git's index.

# usage: repository SOURCE DESTINATION
repository() {
  local source=$1 destination=$2 path target file held as
  local untracked=()
  read -r held as < "$fixtures/held-as"
  mkdir -p "$(dirname "$destination")"
  cp -r "$source" "$destination"
  chmod -R u+w "$destination"
  find "$destination" -name "*.$held" -exec sh -c 'mv "$1" "${1%."$2"}.$3"' _ {} "$held" "$as" \;
  find "$destination" -name '*.nix' -exec sed -i "s|@outside@|$outside|g" {} +
  if [ ! -f "$destination/shikumi.nix" ]; then
    cp "$planted/shikumi.nix" "$destination/shikumi.nix"
  fi
  # grep finds nothing in most repositories, which isn't a failure.
  { grep -rl --include='*.nix' -F './built.nix' "$destination" || true; } | while read -r file; do
    cp "$planted/built.nix" "$(dirname "$file")/built.nix"
  done
  if [ -f "$destination/links" ]; then
    while read -r path target; do
      ln -s "${target//@outside@/$outside}" "$destination/$path"
    done < "$destination/links"
    rm "$destination/links"
  fi
  if [ -f "$destination/untracked" ]; then
    mapfile -t untracked < "$destination/untracked"
    rm "$destination/untracked"
  fi
  shikumi lock pin nixpkgs "file://$nixpkgs" --hash "$nixpkgs_hash" --root "$destination" > /dev/null
  repository_git "$destination" init -q
  repository_git "$destination" add -A
  if [ "${#untracked[@]}" -gt 0 ]; then
    repository_git "$destination" rm -q --cached -- "${untracked[@]}"
  fi
  repository_git "$destination" commit -q -m fixture
}

# git with the identity and branch every repository the checks make is committed under.
repository_git() {
  local directory=$1
  shift
  command git -C "$directory" -c user.name=shikumi -c user.email=shikumi@invalid \
    -c init.defaultBranch=main "$@"
}
