---
name: git-delivery
description: >-
  Plan and carry out a safe Git delivery from branch preparation through commit,
  push, pull request, CI, and release handoff. Use when asked to deliver a
  feature/fix, prepare a commit or PR, or release a batch of changes. Discover
  the repository's workflow instead of assuming branch names, ticket systems,
  or merge policies.
---

# Git delivery

Coordinate Git tasks without assuming every repository uses the same flow.
Treat repository instructions, remote configuration, branch protections, and
the user's explicit authorization as the source of truth.

## Inspect before acting

1. Check the repository, current branch, remotes, `git status`, recent history,
   and the diff. Preserve unrelated local changes; do not reset or discard work.
2. Read `AGENTS.md`, release docs, CI configuration, and project scripts for
   branch bases, naming, required checks, PR templates, and release steps.
3. Determine whether the request authorizes only preparation, or also commit,
   push, PR creation, merge, tagging, or publication. Do not infer permission
   for a later step from permission for an earlier one.

## Branch and changes

- Use the repository's documented base branch and branch naming. If unclear,
  ask rather than assuming `main`, `develop`, or a particular Git flow.
- Keep changes focused. Inspect the final diff and stage only intended files.
- Follow the `conventional-commits` skill for commit messages. Do not commit,
  amend, force-push, or push unless explicitly authorized.
- Before a commit or delivery claim, run the `testing-quality-gate` skill for
  every repository changed.

## Pull requests

Before opening a PR, verify the current branch and commit range, ensure there is
not already an open PR for the same head, and review the complete diff. Use the
repository's target branch and template. A useful description explains:

- **Context:** the problem or request (link a ticket only if one exists).
- **Changes:** meaningful changes and why they were made.
- **Validation:** checks run and any limitations.
- **Compatibility:** breaking changes and migration notes, when applicable.

Use an accurate title based on the change. Do not invent ticket IDs, test
results, or requirements. After opening, report the PR link and monitor required
checks if requested.

Do not merge unless the user authorized merging. Verify the head, mergeability,
checks, and actionable review threads first. For multiple related PRs, use
`pr-merge-sequence` so remaining branches are rebased and revalidated after each
merge.

## Release handoff

Discover the project's release process; do not impose a release-branch pattern.
Confirm the intended contents and target, run the quality gate, and use
`semantic-versioning` to select or validate the version. Prepare a release PR or
notes only when requested. Creating tags, publishing artifacts, or deploying
requires explicit authorization.

## Final report

State what was completed, the branch/commit/PR references, which checks actually
ran, what remains, and any blocker. Never say a PR is ready or a release is
published based on an unverified or cached result.
