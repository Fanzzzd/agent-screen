#!/usr/bin/env bash
# Tests release-decision.sh on a scratch repository shaped like main: a
# release pull request merged with a merge commit (as #231 was, 50cde497),
# squashed, followed by other pushes, and a release that never published.
set -euo pipefail

decide="$(cd "$(dirname "$0")" && pwd)/release-decision.sh"
repo="$(mktemp -d)"
trap 'rm -rf "$repo"' EXIT
cd "$repo"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com
unset GITHUB_OUTPUT
git init -q -b main

set_version() {
  mkdir -p apps/macos
  printf '[package]\nname = "agent-screen"\nversion = "%s"\n\n[dependencies]\nfoo = { version = "9.9.9" }\n' "$1" > apps/macos/Cargo.toml
  git add -A
}
work() {
  echo "$1" >> notes.txt
  git add -A
  git commit -qm "$1"
}

failed=0
expect() { # <name> <want> <event> <before> <head>
  local got
  got="$("$decide" "$3" "$4" "$5" | paste -sd' ' -)"
  if [[ "$got" == "$2" ]]; then
    echo "ok   $1"
  else
    echo "FAIL $1: want '$2', got '$got'"
    failed=1
  fi
}

set_version 0.1.40
git commit -qm "chore(release): Agent Screen 0.1.40"
git tag v0.1.40
work "fix(app): something"
fix="$(git rev-parse HEAD)"

# The release branch, merged with a merge commit.
git checkout -qb release
set_version 0.1.41
git commit -qm "chore(release): Agent Screen 0.1.41"
git checkout -q main
work "fix(app): something else"
before_merge="$(git rev-parse HEAD)"
git merge -q --no-ff release -m "Merge pull request #231 from Fanzzzd/release" -m "chore(release): Agent Screen 0.1.41"
merge="$(git rev-parse HEAD)"

expect "an ordinary push, version tagged" "version=0.1.40 decision=none" push "$(git rev-parse "$fix^")" "$fix"
expect "a release merged with a merge commit" "version=0.1.41 decision=release" push "$before_merge" "$merge"
expect "the same, before unknown" "version=0.1.41 decision=release" push 0000000000000000000000000000000000000000 "$merge"
expect "a dispatch" "version=0.1.40 decision=release" workflow_dispatch "" "$fix"

work "docs: after the merge"
expect "a push after a release that never published" \
  "version=0.1.41 decision=unpublished bumped_in=$merge" push "$merge" HEAD

git tag v0.1.41 "$merge"
expect "a push after the release published" "version=0.1.41 decision=none" push "$(git rev-parse HEAD^)" HEAD

# A squashed release pull request: one commit on main changes the version.
before_squash="$(git rev-parse HEAD)"
set_version 0.1.42
git commit -qm "chore(release): Agent Screen 0.1.42"
expect "a squashed release" "version=0.1.42 decision=release" push "$before_squash" HEAD

# A push of several commits, the release among them.
git reset -q --hard "$before_squash"
work "fix(app): first of two"
set_version 0.1.42
git commit -qm "chore(release): Agent Screen 0.1.42"
work "fix(app): last of two"
expect "a push of three commits with the release in the middle" "version=0.1.42 decision=release" push "$before_squash" HEAD

exit "$failed"
