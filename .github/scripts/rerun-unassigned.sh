#!/usr/bin/env bash
# Reruns a finished workflow run when every job that failed or was
# cancelled did so because GitHub found no runner for it, which GitHub
# says in the job's annotation. A test that failed, a step that failed and
# a run cancelled by a newer push are left alone, and so is a pull
# request's run once the pull request has a newer one: its rerun would
# join the pull request's concurrency group and cancel the newer run.
#
# A rerun started with GITHUB_TOKEN fires workflow_run again when it
# finishes (canonical/snapd's bot attempts 2 to 5 of Tests run
# 44635926113, 19 to 20 November, each followed a workflow_run of the
# attempt before), so MAX_ATTEMPTS is what bounds the reruns.
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

IFS=$'\t' read -r latest_attempt event head_sha head_branch workflow_id < <(
  gh api "repos/$repo/actions/runs/$run" --jq '[.run_attempt, .event, .head_sha, .head_branch, .workflow_id] | @tsv')
attempt="${ATTEMPT:-$latest_attempt}"
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
if [[ "$event" == pull_request ]]; then
  newest="$(gh api --method GET "repos/$repo/actions/workflows/$workflow_id/runs" \
    -f event=pull_request -f branch="$head_branch" -f per_page=1 --jq '.workflow_runs[0].head_sha')"
  if [[ "$newest" != "$head_sha" ]]; then
    echo "Run $run is for $head_sha, but $head_branch has a newer run, for $newest; not rerunning."
    exit 0
  fi
fi
echo "Run $run attempt $attempt: $stuck job(s) never got a runner; rerunning them."
if [[ "${DRY_RUN:-0}" == 1 ]]; then
  echo "(dry run)"
  exit 0
fi
gh run rerun "$run" --failed --repo "$repo"
