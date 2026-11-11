---
name: pr-merge-sequence
description: Safely merge one or more GitHub pull requests while keeping dependent branches current. Use whenever the user asks to merge multiple PRs, rebase PR branches before or after a merge, sequence merges, or says to merge normally rather than squash/rebase.
---

# PR merge sequence

Use this workflow when several open pull requests must be merged without letting later PRs drift behind the base branch.

## Before changing branches

1. Confirm the repository, current branch, and `git status`. Preserve any unrelated local work; do not reset or discard it.
2. List the requested PRs and inspect their base/head branches, draft state, mergeability, latest commit, checks, reviews, inline threads, and unresolved findings.
3. Refresh the base branch from the remote. Read applicable repository instructions and the user's merge/rebase preferences.
4. Do not mark a draft ready or merge any PR until the user has authorized that state change.

## Rebase and merge loop

1. Before the first merge, rebase every requested open PR branch onto the current remote base branch. Push rewritten history with `--force-with-lease`, never plain `--force`. If a rebase conflicts or would overwrite unexpected work, stop and resolve carefully rather than dropping commits.
2. For the next PR in the user-specified order, verify its remote head matches the rebased commit, all required checks are green, and no unresolved valid review finding remains. Inspect new comments after each push/rebase; reply and resolve findings according to the user's established workflow. Do not dismiss a security finding merely to get a green review.
3. Merge with a normal merge commit (for GitHub CLI, `gh pr merge <number> --merge`). Never use squash or rebase merge when the user requires normal merge commits.
4. After each merge, fetch the new base and immediately rebase every remaining requested PR branch onto it. Push each updated branch with `--force-with-lease` and wait for checks/reviews on the new heads before the next merge.
5. Repeat until the requested sequence is complete. Do not merge unrelated PRs just because they are open or green.

## Stop conditions

Pause before merging if a required check fails or is still running, a new actionable finding is unresolved, the PR is not mergeable, the head changed unexpectedly, or the rebase has conflicts. Report the concrete blocker and next safe action. A false positive should be explained with evidence and resolved; a valid finding should be fixed, tested, pushed, and re-reviewed.

## Finish

Verify each requested PR's merged state and merge commit, confirm all surviving branches include the latest base, and report the final heads/checks. If the work included merges, preserve the sequence in the project progress record when the user requested that record be maintained.
