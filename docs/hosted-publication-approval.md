# Hosted review: separate publication approval

Status: **implemented and verified locally against protocol-level fakes**; not
yet exercised against the real GitHub API. Released as a breaking change in the
first v5 release; v4 Actions publish directly.

The user decided that both the GitHub App and the Action require a separate
approval for each publication, and that the operator chooses to approve through
GitHub or the CLI.

## Decisions

- **Inspection surface:** a neutral `Cururu review` Check Run on the PR head
  (App and Action) and the run's job summary (Action) show the canonical
  preview: summary comment, every inline comment with its location, and the
  formal review body. Nothing is commented on the PR before approval. The check
  and summary are visible to anyone who can read the repository's checks; this
  exposure was accepted in exchange for inspecting and approving inside GitHub.
- **Digest binding:** the candidate digest is the SHA-256 of that exact preview.
  Every approval names the digest or a check bound to it, so an approval
  authorizes only inspected content. The CLI recomputes the digest from the
  check text and ignores checks whose text does not match the digest they claim,
  so a forged check cannot substitute other content.
- **GitHub App approval:** the check's **Publish review** button
  (`check_run.requested_action`), a `/cururu approve <64-hex digest>` PR comment,
  or `cururu approve-hosted-review`, which posts that comment as the operator.
  The approver must have write, maintain or admin permission.
- **Action approval:** a `publish` job gated by a GitHub Environment
  (`cururu-publish` by default) with required reviewers. The job refuses to
  publish unless the environment requires reviewers and the run has an approval
  for it. `cururu approve-hosted-review` approves the waiting deployment with a
  `cururu-candidate:<digest>` comment, and the job refuses if any approval names
  a different candidate. The candidate file must match the review job's
  `candidate_digest` output.
- **Exemptions:** the diff-limit notice is still posted without approval (it
  contains no code or review content). Mention answers are posted directly,
  because a write-authorized collaborator requested them. The truncation
  diagnostic, which may contain model-written recommendations, is shown as a
  check instead of a comment.

## Lifecycle (GitHub App)

`pending` → `publishing` (atomic claim; only one approval wins) → `published`,
`invalidated` (head or trusted configuration changed; nothing published) or
`uncertain` (a write failed or a restart interrupted publication; never
retried). A newer candidate for the same PR marks older pending ones
`superseded`. Pending candidates expire after seven days; records are pruned
after thirty days. Pressing the button of a superseded or expired candidate
closes its check without publishing.

## Revalidation

Before the first write, publication rechecks that the candidate belongs to the
PR, that the trusted repository and shared configuration content is unchanged
(the base commit itself may move), and that the PR head is unchanged. The head
is checked again before submitting the formal review and before the summary.
Analyzer evidence is not re-collected: the approval authorizes the inspected
content, not a re-review.

## Verification

- `tests/webhook_review_smoke.py`: signed webhooks, real SQLite queue, fake
  GitHub/LLM. Preparing posts no comments; approvals by an account without write
  access and by an unknown digest publish nothing; a valid `/cururu approve`
  publishes exactly the previewed summary; a duplicate approval publishes
  nothing more; approving after a new push invalidates without publishing.
- `tests/hosted_review_smoke.py`: Action `review` posts no comments and outputs
  the digest; `publish-approved-review` refuses an unprotected environment, a
  missing approval, an approval naming another candidate, a tampered candidate
  file and a moved head; the valid case publishes the previewed summary.
- `tests/hosted_approval_pty.py`: CLI refuses forged checks, cancellation and
  piped input; approves the App by comment and the Action by environment.
- Unit tests cover digest coverage, localization, the size limit, approval
  parsing, candidate state transitions, expiry and restart recovery.

## Limitations

- GitHub cannot condition a POST on the PR revision; a push between the final
  check and a write can still make a publication stale.
- The existing comment helpers retry some requests internally; a lost response
  during approved publication could duplicate a comment. Inline comments and
  the summary are reconciled/upserted, which limits duplicates.
- Approvals happen in GitHub; Cururu verifies them but cannot prove the approver
  actually read the preview.
- Previews over 65 000 bytes cannot be approved; reduce the PR or finding scope.
- Real GitHub behavior (check text normalization, environment and approval APIs
  with `GITHUB_TOKEN`, App button delivery) has not been tested against GitHub.
