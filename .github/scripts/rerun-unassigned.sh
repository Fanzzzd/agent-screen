#!/usr/bin/env bash
# Reruns a finished workflow run when every job that failed or was
# cancelled did so because GitHub found no runner for it, which GitHub
# says in the job's annotation. A test that failed, a step that failed and
# a run cancelled by a newer push are left alone.
#
# Usage: rerun-unassigned.sh <run id>
# Env: GH_TOKEN, GITHUB_REPOSITORY; MAX_ATTEMPTS (default 3: two reruns);
# DRY_RUN=1 prints the verdict without rerunning; ATTEMPT picks an older
# attempt, with DRY_RUN, to check the verdict on past runs.
set -euo pipefail

run="$1"
repo="${GITHUB_REPOSITORY:?}"
max="${MAX_ATTEMPTS:-3}"
unassigned='was not acquired by Runner'

attempt="${ATTEMPT:-$(gh api "repos/$repo/actions/runs/$run" --jq .run_attempt)}"
jobs="$(gh api --paginate "repos/$repo/actions/runs/$run/attempts/$attempt/jobs?per_page=100" \
  --jq '.jobs[] | select(.conclusion == "failure" or .conclusion == "cancelled" or .conclusion == "timed_out") | "\(.id)\t\(.name)"')"

if [[ -z "$jobs" ]]; then
  echo "Run $run attempt $attempt: no failed or cancelled job."
  exit 0
fi

stuck=0
other=0
while IFS=$'\t' read -r id name; do
  messages="$(gh api "repos/$repo/check-runs/$id/annotations" --jq '[.[] | select(.annotation_level == "failure") | .message] | join(" / ")')"
  if [[ "$messages" == *"$unassigned"* ]]; then
    echo "no runner: $name"
    stuck=$((stuck + 1))
  else
    echo "other: $name: ${messages:-no annotation}"
    other=$((other + 1))
  fi
done <<<"$jobs"

if ((other > 0)); then
  echo "Run $run attempt $attempt: $other job(s) failed for another reason; not rerunning."
  exit 0
fi
if ((attempt >= max)); then
  echo "::warning::Run $run had no runner on attempt $attempt, the last one allowed."
  exit 0
fi
echo "Run $run attempt $attempt: $stuck job(s) never got a runner; rerunning them."
if [[ "${DRY_RUN:-0}" == 1 ]]; then
  echo "(dry run)"
  exit 0
fi
gh run rerun "$run" --failed --repo "$repo"
