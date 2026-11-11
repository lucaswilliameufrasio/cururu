# Hosted review: separate publication approval

Status: **implementation pending**. The user requested that both the GitHub
App and Action require a separate approval for each publication, and that the
operator can choose approval through GitHub or the CLI. The current hosted
implementation still publishes automatically; do not treat it as satisfying
this requirement until the new journeys are implemented and tested.

## Required journeys

1. The operator authorizes disclosure to the configured model. Preparing a
   review does not itself authorize publication.
2. The hosted review produces a complete, inspectable candidate, bound to its
   repository, PR, head, effective configuration, evidence, and exact rendered
   publication. An incomplete review cannot be approved as complete.
3. The operator chooses either an authenticated GitHub interaction or an
   interactive CLI confirmation. Both must identify the same candidate and
   authorize exactly that candidate; neither is a blanket repository setting.
4. The publisher rechecks authorization and the current snapshot before any
   review, summary, inline comment, diagnostic, or other publication. Changed
   input invalidates approval; the operator must inspect and approve again.
5. Duplicate requests, worker restarts, and uncertain remote outcomes must not
   silently create repeated publications or turn failure into approval. The
   GitHub API does not support an atomic snapshot check and POST, so the
   remaining race must be disclosed.

## Design work and verification still required

- Design durable candidate storage and retrieval for both the long-running App
  and the short-lived Action. Keep source and review data private; the public PR
  must not receive the candidate before approval. Bind the approval to the
  actual rendered candidate as well as its snapshot, not merely to a PR number.
- Define authenticated, permission-checked GitHub approval and a CLI path that
  can securely retrieve and approve the *same* candidate. An editable receipt,
  issue comment, artifact, or caller-provided hash alone is not proof of an
  authorized approval. Avoid an implicit dependency on a specific desktop.
- Define idempotent state transitions, concurrent approval/publication,
  expiration, cancellation, permission changes, and recovery after an
  interrupted or uncertain POST. Do not automatically retry an uncertain POST.
- Cover signed webhook and Action entry points with real local persistence and
  protocol-level service fakes. Verify no approval, denial, unauthorized actor,
  candidate tampering, changed PR/config/evidence, duplicate approval,
  restart, and each successful approval interface. Verify that diagnostic and
  limit-notice branches cannot bypass the boundary.
- Replace existing documentation and tests that assume immediate hosted
  publication only after the replacement flows are operational. Do not weaken
  existing CLI/MCP confirmation or enable evaluator filtering.
