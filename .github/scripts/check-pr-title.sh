#!/usr/bin/env bash
# Checks a pull request title against the commit convention in
# CONTRIBUTING.md. Squash merging makes the title the commit's subject.
# Usage: check-pr-title.sh "fix(mcp): keep the window on the display"
set -euo pipefail

title="${1-}"
types='feat|fix|perf|refactor|test|docs|build|ci|chore|revert'
scopes='app|mcp|model|e2e|website|docs|release'
# The summary starts in lowercase, or with the name Agent Screen, as the
# release title does.
pattern="^($types)(\\(($scopes)\\))?!?: ([^A-Z[:space:]]|Agent Screen ).*[^.[:space:]]\$"

problem=''
if ! [[ "$title" =~ $pattern ]]; then
  problem='It must read type(scope): summary, with the summary in lowercase (Agent Screen may start it) and no period at the end.'
elif ((${#title} > 72)); then
  problem="It is ${#title} characters long; the limit is 72."
fi

if [[ -n "$problem" ]]; then
  cat >&2 <<MSG
The pull request title "$title" doesn't follow the commit convention.
$problem

Squash merging makes the title the commit's subject on main.
Types: ${types//|/, }
Scopes, optional: ${scopes//|/, }
Add ! after the type or scope for a breaking change to the control
protocol or an MCP tool's schema.

Examples:
  fix(mcp): keep a placed window on the display
  feat(mcp)!: rename place_window's frame arguments
  docs: explain the countdown sign

See CONTRIBUTING.md, Commits and pull requests.
MSG
  exit 1
fi
echo "Title OK: $title"
