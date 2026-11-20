# CI reliability, November 2026

A red CI on Agent Screen was usually not a defect. Of 36 failed CI attempts from 10 to 19 November, 32 were jobs that GitHub cancelled after 15 minutes because no macOS arm64 runner took them. Four were two test races and one was a runner that lost contact. None was a defect in the code under test. Pull request [#238](https://github.com/Fanzzzd/agent-screen/pull/238) acts on these numbers. This file records the measurements and what changed because of them.

The earlier study of the release pipeline, in Chinese, is [macos-release-ci.zh.md](macos-release-ci.zh.md) (2026-11-14, sources cited). This file checks its conclusions against a larger sample.

## Method

The data was read with `gh api` on 2026-11-20:

- `actions/workflows/<id>/runs`: the last 100 runs of each workflow, 200 for CI.
- `actions/runs/<id>/attempts/<n>/jobs`: every attempt's jobs, with created, started and completed times, runner and steps.
- `check-runs/<job id>/annotations`: GitHub's own reason for each job that failed without running a step.
- `actions/jobs/<id>/logs`: the log of every job with a failed step, read for the failing test or command.

A job "never assigned" has no step, no runner name, and the annotation "The job was not acquired by Runner of type hosted even after multiple attempts". All 45 such jobs carry it. One job without steps carries "The hosted runner lost communication with the server" instead. A flaky test is one that failed and passed on a rerun of the same commit, or whose fix changed only the test.

| Workflow        | Runs | From       | To         | Attempts that ran | Failed attempts |
| --------------- | ---: | ---------- | ---------- | ----------------: | --------------: |
| CI              |  200 | 2026-11-10 | 2026-11-19 |               213 |              36 |
| Release         |  100 | 2026-10-28 | 2026-11-19 |                29 |               6 |
| Prepare release |  100 | 2026-10-26 | 2026-11-19 |                82 |               5 |
| PR title        |  100 | 2026-11-13 | 2026-11-19 |                88 |               6 |

Seven CI runs on the release branch's pull request have no jobs: prepare-release approves them and their jobs skip that branch. They are left out of the CI counts.

## Findings

### Why CI attempts failed

| Cause                                  | CI attempts | Jobs                                                     |
| -------------------------------------- | ----------: | -------------------------------------------------------- |
| No runner assigned                     |          31 | rust 19, build-release 16, starts-on-macos-15 8          |
| Flaky test                             |           3 | rust 4                                                   |
| Flaky test and no runner, same attempt |           1 | (counted above)                                          |
| Runner lost contact                    |           1 | build-release 1                                          |
| Real defect                            |           0 |                                                          |
| Total                                  |          36 | 48                                                       |

30 of 199 CI runs (15%) failed on their first attempt. 24 runs needed a manual rerun; 4 of them needed two.

### No runner assigned

GitHub did not assign a runner to 43 of 544 macOS job attempts in CI (7.9%): 35 of 374 on `macos-26` (9.4%) and 8 of 170 on `macos-15` (4.7%). Every one was cancelled 15 to 19 minutes after it was queued. Their annotations also say "Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times." No Linux job was affected.

The failures cluster on days of low capacity:

| Day (UTC)  | macOS job attempts | Not assigned |
| ---------- | -----------------: | -----------: |
| 2026-11-10 |                 44 |            3 |
| 2026-11-11 |                 69 |            0 |
| 2026-11-12 |                 91 |            7 |
| 2026-11-13 |                 60 |            9 |
| 2026-11-14 |                 12 |            0 |
| 2026-11-15 |                 81 |            0 |
| 2026-11-16 |                  6 |            0 |
| 2026-11-18 |                 94 |            7 |
| 2026-11-19 |                 87 |           17 |

A run with three macOS jobs fails if any one is not assigned, so each macOS job a pull request runs adds to its chance of a false red. Reruns usually worked. Of the 26 runs whose first attempt hit an unassigned job, the second attempt hit one again in 4 and the third in 2. Those reruns were manual, minutes to hours later.

Release lost two attempts of one dry run to the same cause (run [43481950920](https://github.com/Fanzzzd/agent-screen/actions/runs/43481950920)).

### Flaky tests

- `control::server::tests::the_sweep_ends_only_an_mcp_peer_launchd_adopted_and_still_the_same_process` failed 3 times: the test checked the peer list before the peer dropped. Commit eec556c fixed the test.
- `control::server::tests::a_connection_the_host_opens_after_its_pipe_ended_serves_nothing` failed once with `BrokenPipe`: the host ended the connection before the test wrote its line, and the test unwrapped the write. #238 accepts a broken pipe there, since nothing is served either way.
- `crates/mcp/tests/pipe.rs`, `killing_the_parent_ends_the_pipe_while_stdin_stays_open`, never failed in CI but failed twice in local runs right after a rebuild ("the pipe outlived its parent by 5 s"). The test killed the pipe's parent after a fixed 500 ms. A pipe that had not yet run its first line then saw launchd as its parent and never watched it. With the wait at 0 ms the old test fails every time. #238 waits until the pipe connects to the app, which happens after the watch starts; the new test passed 10 runs in a row.
- `crates/mcp/tests/input_queue_protocol` failed locally on a stale binary in the shared target directory that several worktrees build into. CI has its own target directory, so this is not a CI flake.

### Durations and cache

Successful job attempts, in minutes:

| Job                   | Runner  | Median |  p90 |  Max | Queue p90 | Queue max |
| --------------------- | ------- | -----: | ---: | ---: | --------: | --------: |
| CI rust               | macos-26 |    2.8 |  3.7 | 17.8 |       5.0 |      13.1 |
| CI build-release      | macos-26 |    2.7 |  3.5 | 12.4 |       3.3 |      10.1 |
| CI starts-on-macos-15 | macos-15 |    0.1 |  0.1 |  0.2 |       5.0 |      11.1 |
| CI ubuntu jobs        | ubuntu  |    0.1 |  0.1 |  0.3 |       0.0 |       0.5 |
| Release build         | macos-26 |   23.1 | 25.6 | 30.0 |       0.3 |      10.6 |

A pull request's CI, from the first job queued to the last job done, took a median 3.2 minutes and a p90 of 13.0 over 95 runs that passed on the first attempt. A run with an unassigned job took at least 15 minutes more, plus the rerun.

CI's `rust` job restored its cache in about 30 seconds and spent about 35 seconds in clippy and 70 in building the tests. The Release build found no cache ("No cache found"): its `Check` step took 15.6 minutes and the build and notarization 7.7. The repository held 10 caches of 1.0 to 1.2 GB, 11.4 GB in total, over GitHub's 10 GB limit. Five belonged to one pull request (#223), two to the release branch and three to main, so pull request caches pushed main's out.

### Release, prepare-release and the PR title check

- Release: 4 attempts failed when notarytool refused the App Store Connect key, after 10.6 to 16.2 minutes of checks. Two logs say "A required agreement is missing or has expired" (403). The two before commit dab5e39 hid notarytool's message.
- Prepare release: 5 runs failed with knope's `no_release`: changesets named a package other than `default`. Commit 6cf5c9d (#125) fixed the changesets and added the CI check for them. This was a real defect, and the workflow caught it.
- PR title: 6 runs failed on titles that broke the convention. The check did its job.

Node 20 warnings came from `actions/upload-artifact@v4` and `actions/download-artifact@v4`, in CI and Release.

## Changes in #238

| Finding                                                     | Change                                                                                                                                                                                                                                                                                                                                            |
| ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Each macOS job is another chance of no runner               | A pull request runs one macOS job instead of three: clippy and the tests. The release build and its start on macOS 15 run on pushes to main and release, which still gate the release pull request.                                                                                                                                              |
| Linux runners were always assigned                          | fmt, the changeset checks, the website check, and clippy and the tests of the model crate run in one Linux job. A Linux build also keeps the model crate free of OS calls.                                                                                                                                                                         |
| Reruns of unassigned jobs passed                            | `rerun-unassigned.yml`, on `workflow_run` of CI and Release, reruns a run up to twice when every failed or cancelled job carries the "not acquired by Runner" annotation. A failed step, a lost runner, a run a newer push cancelled, and a pull request's run once the pull request has a newer run are left alone. A rerun started with `GITHUB_TOKEN` fires `workflow_run` again: in canonical/snapd, which reruns the same way, the bot started attempts 2 to 5 of Tests run 44635926113, each seconds after a `workflow_run` of the attempt before. So the second rerun happens. Checked in dry-run mode against past attempts; its verdict matched the table below for each. |
| Two test races                                              | Both tests fixed; no test is retried.                                                                                                                                                                                                                                                                                                             |
| Cache over the limit, Release without cache                 | Only main saves the Rust cache. CI's macOS job and the Release build share it (`shared-key: rust`); Release only reads it.                                                                                                                                                                                                                         |
| No `timeout-minutes` anywhere (default 360)                  | Every job has one, set from the durations above.                                                                                                                                                                                                                                                                                                  |
| Pushes to a pull request ran side by side                   | A new push to a pull request cancels its older CI and title runs. Runs on main and release always finish.                                                                                                                                                                                                                                          |
| Release failed on credentials after 11 to 16 minutes        | The build job checks every secret and asks notarytool first, then imports the certificate, then runs the checks. `release-credentials.yml` runs the same check weekly on Linux through Apple's Notary API, plus the certificate's expiry and the deploy keys.                                                                                     |
| Signing secrets are repository secrets any branch can read  | The release jobs use a `release` environment. Its main-only rule cannot match a `pull_request` event, whose ref is the pull request's merge ref, so Release now starts from the push to main. Its first job releases when the push changed the version to one with no tag, which holds for a squash and for a merge commit (the last three releases were merge commits), and fails loudly when a version on main never published. The owner creates the environment and moves the secrets (steps in #238).                                              |
| Unpinned actions, Node 20 warnings                           | Every action is pinned to a commit: checkout v5.1.0, upload-artifact v7.0.2, download-artifact v8.0.2, rust-cache v2.9.3, knope action v2.1.2. All run on Node 24 or are composite.                                                                                                                                                              |

Notarization stays off pull requests.

## Results

CI runs on #238, all green; later runs are linked in #238. Wall time is from the run's start to its last job's end. Runs 1 and 2 restored main's existing cache. Run 3 missed it because its runner image had Rust 1.100.0 installed besides the pinned 1.98.1, and rust-cache hashes every installed toolchain into the key. Commit 05bc593 uninstalls every toolchain but the pinned one before the cache step, which changed the key, so runs 4 and 5 built cold: a pull request cannot save a cache, and main has not saved one under the new key yet.

| Run | Commit | Cache | macOS job queued | Clippy + tests | Wall |
| --- | --- | --- | ---: | ---: | ---: |
| 1 [44581347596](https://github.com/Fanzzzd/agent-screen/actions/runs/44581347596) | 7ca0ea8 | warm | 0.0 min | 2.8 min | 3.9 min |
| 2 [44582221116](https://github.com/Fanzzzd/agent-screen/actions/runs/44582221116) | 0e42f5b | warm | 5.0 min | 2.3 min | 8.0 min |
| 3 [44583786209](https://github.com/Fanzzzd/agent-screen/actions/runs/44583786209) | 816a4b7 | missed, image toolchain | 0.0 min | 10.6 min | 10.9 min |
| 4 [44586289383](https://github.com/Fanzzzd/agent-screen/actions/runs/44586289383) | 05bc593 | cold, new key | 0.0 min | 15.4 min | 15.9 min |
| 5 [44589315857](https://github.com/Fanzzzd/agent-screen/actions/runs/44589315857) | 3fd8521 | cold, new key | 0.0 min | 14.5 min | 15.0 min |

What the cache saves: clippy and the tests took 2.3 to 2.8 minutes warm and 10.6 to 15.4 minutes cold on the same runner type. Before this change Release always ran cold: its checks took 15.6 minutes. After merge, Release restores main's cache, so its checks should take about as long as CI's warm run.

The Linux job took 1.6 to 2.1 minutes, cold, because a pull request cannot save its cache.

Expected effect on failures, an estimate from the November rates and not yet measured: a pull request now runs one macOS job instead of three. With 7.9% of macOS jobs unassigned, the chance that a pull request run hits one falls from about 22% to about 8%. In the measured runs, 13% of first attempts (26 of 199) hit one, since unassigned jobs clustered in the same runs. The automatic rerun handles most of the rest: a second attempt hit an unassigned job again in 4 of 26 runs. Measure again after a few weeks on main.

The `BrokenPipe` race reproduced in CI on a throwaway branch: the old test with a 50 ms delay before its write failed with `Kind(BrokenPipe)` ([44581461017](https://github.com/Fanzzzd/agent-screen/actions/runs/44581461017)), and the fixed test with the same delay passed ([44585960871](https://github.com/Fanzzzd/agent-screen/actions/runs/44585960871)).

## Every failed job attempt

Times are UTC, job creation. "Min" is minutes from queued to done.

| Created (UTC) | Workflow | Run#attempt | Event | Job | Min | Cause |
| --- | --- | --- | --- | --- | ---: | --- |
| 2026-10-29 09:50 | Prepare release | [40852760103#1](https://github.com/Fanzzzd/agent-screen/actions/runs/40852760103/attempts/1) | push | prepare-release | 0.2 | real defect: knope no_release, changeset key (fixed in 6cf5c9d) |
| 2026-11-04 01:09 | Prepare release | [41752043743#1](https://github.com/Fanzzzd/agent-screen/actions/runs/41752043743/attempts/1) | push | prepare-release | 0.2 | real defect: knope no_release, changeset key (fixed in 6cf5c9d) |
| 2026-11-04 01:15 | Prepare release | [41752745878#1](https://github.com/Fanzzzd/agent-screen/actions/runs/41752745878/attempts/1) | push | prepare-release | 0.2 | real defect: knope no_release, changeset key (fixed in 6cf5c9d) |
| 2026-11-04 01:19 | Prepare release | [41753282317#1](https://github.com/Fanzzzd/agent-screen/actions/runs/41753282317/attempts/1) | push | prepare-release | 0.1 | real defect: knope no_release, changeset key (fixed in 6cf5c9d) |
| 2026-11-04 01:31 | Prepare release | [41754630368#1](https://github.com/Fanzzzd/agent-screen/actions/runs/41754630368/attempts/1) | push | prepare-release | 0.2 | real defect: knope no_release, changeset key (fixed in 6cf5c9d) |
| 2026-11-10 18:04 | CI | [42871361552#1](https://github.com/Fanzzzd/agent-screen/actions/runs/42871361552/attempts/1) | push | rust | 15.0 | runner never assigned |
| 2026-11-10 18:19 | CI | [42874015447#1](https://github.com/Fanzzzd/agent-screen/actions/runs/42874015447/attempts/1) | push | rust | 15.1 | runner never assigned |
| 2026-11-10 18:19 | CI | [42874015447#1](https://github.com/Fanzzzd/agent-screen/actions/runs/42874015447/attempts/1) | push | build-release | 15.1 | runner never assigned |
| 2026-11-12 04:42 | CI | [43156525894#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43156525894/attempts/1) | push | build-release | 51.2 | infra: runner lost contact |
| 2026-11-12 15:56 | CI | [43253148452#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43253148452/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-12 16:17 | CI | [43257028748#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43257028748/attempts/1) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-12 16:34 | CI | [43260083776#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43260083776/attempts/1) | push | rust | 15.0 | runner never assigned |
| 2026-11-12 16:34 | CI | [43260083776#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43260083776/attempts/1) | push | build-release | 15.0 | runner never assigned |
| 2026-11-12 16:34 | CI | [43260122640#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43260122640/attempts/1) | push | rust | 15.0 | runner never assigned |
| 2026-11-12 16:34 | CI | [43260122640#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43260122640/attempts/1) | push | build-release | 15.0 | runner never assigned |
| 2026-11-12 16:47 | CI | [43262442025#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43262442025/attempts/1) | push | build-release | 17.0 | runner never assigned |
| 2026-11-13 14:52 | CI | [43446908225#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43446908225/attempts/1) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-13 14:52 | CI | [43446908225#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43446908225/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-13 15:27 | CI | [43452765594#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43452765594/attempts/1) | push | build-release | 15.0 | runner never assigned |
| 2026-11-13 15:27 | CI | [43452765594#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43452765594/attempts/1) | push | rust | 15.0 | runner never assigned |
| 2026-11-13 16:33 | CI | [43463974051#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43463974051/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-13 16:33 | CI | [43463974051#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43463974051/attempts/1) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-13 16:53 | CI | [43467237986#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43467237986/attempts/1) | push | rust | 15.0 | runner never assigned |
| 2026-11-13 16:53 | CI | [43467281718#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43467281718/attempts/1) | push | rust | 15.0 | runner never assigned |
| 2026-11-13 17:10 | CI | [43467281718#2](https://github.com/Fanzzzd/agent-screen/actions/runs/43467281718/attempts/2) | push | rust | 15.0 | runner never assigned |
| 2026-11-13 17:41 | Release | [43475304079#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43475304079/attempts/1) | pull_request | build | 10.6 | Apple credentials: notarytool refused the key |
| 2026-11-13 17:54 | Release | [43475304079#2](https://github.com/Fanzzzd/agent-screen/actions/runs/43475304079/attempts/2) | pull_request | build | 16.0 | Apple credentials: notarytool refused the key |
| 2026-11-13 18:22 | Release | [43481950920#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43481950920/attempts/1) | workflow_dispatch | build | 15.0 | runner never assigned |
| 2026-11-13 18:38 | Release | [43481950920#2](https://github.com/Fanzzzd/agent-screen/actions/runs/43481950920/attempts/2) | workflow_dispatch | build | 15.0 | runner never assigned |
| 2026-11-13 18:55 | Release | [43481950920#3](https://github.com/Fanzzzd/agent-screen/actions/runs/43481950920/attempts/3) | workflow_dispatch | build | 16.2 | Apple credentials: notarytool refused the key |
| 2026-11-15 03:12 | Release | [43671304547#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43671304547/attempts/1) | workflow_dispatch | build | 14.7 | Apple credentials: notarytool refused the key |
| 2026-11-15 10:31 | PR title | [43704490993#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43704490993/attempts/1) | pull_request | title | 0.1 | check working: title broke the convention |
| 2026-11-15 17:05 | PR title | [43739133269#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43739133269/attempts/1) | pull_request | title | 0.1 | check working: title broke the convention |
| 2026-11-15 17:05 | PR title | [43739167589#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43739167589/attempts/1) | pull_request | title | 0.1 | check working: title broke the convention |
| 2026-11-15 17:09 | PR title | [43739505583#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43739505583/attempts/1) | pull_request | title | 0.1 | check working: title broke the convention |
| 2026-11-15 18:22 | PR title | [43746396762#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43746396762/attempts/1) | pull_request | title | 0.1 | check working: title broke the convention |
| 2026-11-15 18:23 | PR title | [43746474085#1](https://github.com/Fanzzzd/agent-screen/actions/runs/43746474085/attempts/1) | pull_request | title | 0.1 | check working: title broke the convention |
| 2026-11-18 14:50 | CI | [44328725439#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44328725439/attempts/1) | push | build-release | 15.0 | runner never assigned |
| 2026-11-18 15:20 | CI | [44334403638#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44334403638/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-18 15:27 | CI | [44335927966#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44335927966/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-18 15:40 | CI | [44338280133#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44338280133/attempts/1) | pull_request | build-release | 17.8 | runner never assigned |
| 2026-11-18 15:40 | CI | [44335927966#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44335927966/attempts/1) | pull_request | starts-on-macos-15 | 19.4 | runner never assigned |
| 2026-11-18 15:48 | CI | [44338901066#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44338901066/attempts/1) | pull_request | starts-on-macos-15 | 15.7 | runner never assigned |
| 2026-11-18 15:54 | CI | [44341080898#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44341080898/attempts/1) | pull_request | rust | 18.1 | runner never assigned |
| 2026-11-19 08:23 | CI | [44487121875#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44487121875/attempts/1) | pull_request | rust | 15.1 | flaky test: the_sweep_ends_only_an_mcp_peer_launchd_adopted... (fixed in eec556c) |
| 2026-11-19 10:30 | CI | [44506256566#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44506256566/attempts/1) | pull_request | rust | 2.3 | flaky test: a_connection_the_host_opens_after_its_pipe_ended_serves_nothing, BrokenPipe (fixed here) |
| 2026-11-19 14:08 | CI | [44540000113#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44540000113/attempts/1) | pull_request | starts-on-macos-15 | 15.0 | runner never assigned |
| 2026-11-19 14:25 | CI | [44543655564#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44543655564/attempts/1) | pull_request | rust | 2.2 | flaky test: the_sweep_ends_only_an_mcp_peer_launchd_adopted... (fixed in eec556c) |
| 2026-11-19 15:04 | CI | [44550740501#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44550740501/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-19 15:04 | CI | [44550740501#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44550740501/attempts/1) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-19 15:21 | CI | [44550740501#2](https://github.com/Fanzzzd/agent-screen/actions/runs/44550740501/attempts/2) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-19 15:21 | CI | [44550740501#2](https://github.com/Fanzzzd/agent-screen/actions/runs/44550740501/attempts/2) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-19 15:37 | CI | [44550740501#3](https://github.com/Fanzzzd/agent-screen/actions/runs/44550740501/attempts/3) | pull_request | rust | 2.5 | flaky test: the_sweep_ends_only_an_mcp_peer_launchd_adopted... (fixed in eec556c) |
| 2026-11-19 15:39 | CI | [44550740501#3](https://github.com/Fanzzzd/agent-screen/actions/runs/44550740501/attempts/3) | pull_request | starts-on-macos-15 | 15.0 | runner never assigned |
| 2026-11-19 15:40 | CI | [44557444378#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44557444378/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-19 15:40 | CI | [44557444378#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44557444378/attempts/1) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-19 15:40 | CI | [44557525062#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44557525062/attempts/1) | pull_request | rust | 15.0 | runner never assigned |
| 2026-11-19 15:40 | CI | [44557525062#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44557525062/attempts/1) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-19 16:07 | CI | [44557444378#2](https://github.com/Fanzzzd/agent-screen/actions/runs/44557444378/attempts/2) | pull_request | starts-on-macos-15 | 16.2 | runner never assigned |
| 2026-11-19 16:08 | CI | [44562715391#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44562715391/attempts/1) | push | rust | 16.2 | runner never assigned |
| 2026-11-19 16:35 | CI | [44567493442#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44567493442/attempts/1) | push | starts-on-macos-15 | 15.0 | runner never assigned |
| 2026-11-19 16:36 | CI | [44568055598#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44568055598/attempts/1) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-19 16:46 | CI | [44567447228#1](https://github.com/Fanzzzd/agent-screen/actions/runs/44567447228/attempts/1) | push | starts-on-macos-15 | 15.0 | runner never assigned |
| 2026-11-19 16:53 | CI | [44568055598#2](https://github.com/Fanzzzd/agent-screen/actions/runs/44568055598/attempts/2) | pull_request | build-release | 15.0 | runner never assigned |
| 2026-11-19 17:12 | CI | [44568055598#3](https://github.com/Fanzzzd/agent-screen/actions/runs/44568055598/attempts/3) | pull_request | starts-on-macos-15 | - | runner never assigned |
