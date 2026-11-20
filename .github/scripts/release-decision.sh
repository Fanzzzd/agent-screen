#!/usr/bin/env bash
# Decides from the repository whether a push to main releases. The commit
# message can't say: a release pull request merged with a merge commit
# reads "Merge pull request #N from Fanzzzd/release", squashed it reads
# "chore(release): Agent Screen x.y.z". What every merge shares is that
# it changes the version in apps/macos/Cargo.toml to one with no tag yet.
#
# Prints, and writes to $GITHUB_OUTPUT when set:
#   decision=release      the push changed the version, and v<version> has no tag
#   decision=none         v<version> is tagged: nothing to release
#   decision=unpublished  v<version> has no tag, and an earlier push set it;
#                         bumped_in= names that push's commit on main
#   version=<version>
#
# Usage: release-decision.sh <event> <before> [<head>]
# <before> is the push's github.event.before; when it isn't a commit here
# (a new branch, a force push), the head's first parent stands in for it.
set -euo pipefail

event="$1"
before="$2"
head="$(git rev-parse "${3:-HEAD}^{commit}")"
manifest=apps/macos/Cargo.toml

version_at() {
  git show "$1:$manifest" 2>/dev/null | awk -F'"' '/^version/ { print $2; exit }'
}

out() {
  echo "$1"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then echo "$1" >> "$GITHUB_OUTPUT"; fi
}

version="$(version_at "$head")"
[[ -n "$version" ]] || { echo "::error::no version in $manifest at $head"; exit 1; }
out "version=$version"

if [[ "$event" == workflow_dispatch ]]; then
  out "decision=release"
  exit 0
fi

if git rev-parse -q --verify "refs/tags/v$version^{commit}" > /dev/null; then
  out "decision=none"
  exit 0
fi

if ! git cat-file -e "$before^{commit}" 2>/dev/null; then
  before="$(git rev-parse "$head^1")"
fi
if [[ "$(version_at "$before")" != "$version" ]]; then
  out "decision=release"
  exit 0
fi

# Walk main back to the push that set this version.
bumped_in=""
for commit in $(git rev-list --first-parent --max-count=500 "$head"); do
  parent="$(git rev-parse -q --verify "$commit^1" || true)"
  if [[ -z "$parent" || "$(version_at "$parent")" != "$version" ]]; then
    bumped_in="$commit"
    break
  fi
done
out "decision=unpublished"
out "bumped_in=$bumped_in"
