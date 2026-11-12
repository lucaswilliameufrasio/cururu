# Auto-approval (opt-in)

Cururu can approve a pull request on its own when the change is clearly low
risk, and ask for a human reviewer otherwise. It is **disabled by default**.
Run `cururu setup auto-approval` to preview a starting policy and check the
repository requirements; `cururu setup auto-approval --write` appends it to
`.cururu.toml` after an interactive confirmation. `cururu status` lists what is
enabled. The policy can be defined once for an organization through the shared
configuration (`[config] base`).

```toml
[auto_approval]
enabled = true
allowed_paths = ["docs/**", "**/*.md"]          # every changed file must match (empty = any)
blocked_paths = ["**/payments/**", ".github/**"] # any match needs a human
max_changed_files = 20
max_changed_lines = 400
block_on = "low"                                  # findings at or above this block
```

## How it decides

Every rule must pass on the **whole pull request at the reviewed head**:

- each changed path (including the source of a rename or deletion) is allowed
  and not blocked;
- no changed file was excluded from review (`review.ignore`);
- the file and line limits hold, and the review covered every file;
- no finding is at or above `block_on`; an unknown severity also blocks;
- when analyzer evidence is enabled, it passed.

The model never approves on its own; its findings can only block. Anything
uncertain (diff over the limit, truncated or failed review) means no approval.
If an earlier review commented and the author fixed it, the next review of the
new head approves only when it finds nothing blocking and the change still
meets every rule.

## What it does

- **Approved:** Cururu submits an `APPROVE` review bound to the head, publishes
  its review and summary directly (the repository's opt-in replaces the
  per-review publication approval), and reports a successful `Cururu triage`
  check. It never merges and never bypasses branch protection.
- **Needs a human:** Cururu posts one short notice, in the configured language,
  listing the reasons and no code. The full review follows the regular
  publication approval. The `Cururu triage` check is neutral.
- **New commits:** Cururu dismisses its own earlier approval and decides again.
- **Approval refused by GitHub:** for example when GitHub Actions may not
  approve pull requests. The decision becomes "needs human review" and the
  notice explains the setting to change: *Settings → Actions → General → Allow
  GitHub Actions to create and approve pull requests*, or use the Cururu GitHub
  App with Pull requests (write). Whether an approval by Actions counts toward
  required reviewers depends on your branch protection.

## Workflow outputs

The Action exposes `auto_approval` (`approved`, `needs_human`, `disabled`,
`not_evaluated`), `auto_approval_reasons` (JSON array with `code` and a
localized `message`) and `pr_url`. Cururu does not talk to chat tools; wire the
outputs to the channel you use. The workflow needs `checks: write` and
`pull-requests: write`. Example job for an incoming webhook (Discord-style;
adapt the payload for Slack, Telegram or others):

```yaml
  notify:
    needs: review
    if: ${{ !cancelled() && needs.review.outputs.auto_approval == 'needs_human' }}
    runs-on: ubuntu-latest
    steps:
      - name: Ask for a human review
        env:
          WEBHOOK_URL: ${{ secrets.REVIEW_WEBHOOK_URL }}
          PR_URL: ${{ needs.review.outputs.pr_url }}
          REASONS: ${{ needs.review.outputs.auto_approval_reasons }}
        run: |
          message="$(jq -r --arg url "$PR_URL" \
            '"Human review needed: \($url)\n" + (map("- " + .message) | join("\n"))' <<<"$REASONS")"
          jq -n --arg content "$message" '{content: $content}' \
            | curl --fail --silent --show-error --connect-timeout 10 --max-time 30 \
              -H 'Content-Type: application/json' -d @- "$WEBHOOK_URL"
```

Add `auto_approval`, `auto_approval_reasons` and `pr_url` to the review job's
`outputs:` (as `${{ steps.review.outputs.<name> }}`) to use them in other jobs.
The GitHub App reports the same decision through the `Cururu triage` check,
which a repository workflow can react to with a `check_run` trigger.
