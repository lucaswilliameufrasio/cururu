# Architecture

> See also: [Glossary](glossary.md) for terms like anchor, base commit, and quality gate, and the [Security Policy](../SECURITY.md) for the trust model.

Cururu has two execution modes. The GitHub Action is **stateless**: one Docker
run reviews one PR and exits. The optional self-hosted GitHub App server persists
webhook deliveries in PostgreSQL or SQLite and runs reviews asynchronously.

## Review pipeline

```
serve (CLI: self-hosted GitHub App)
  -> app.rs         verify HMAC, dedupe X-GitHub-Delivery, enqueue payload
  -> app/db.rs      durable queue, retries, recovery and retention
  -> app/github_app.rs  sign App JWT, mint installation tokens
  -> app.rs worker  route PR events and authorized conversation triggers
  -> review pipeline below (shared with the Action mode)

commands.rs (CLI: review)
  -> config/        env vars + optional shared TOML + local TOML -> AppConfig
  -> github.rs      fetch PR diff, review comments, context files via GitHub API
  -> diff.rs        parse unified diff, chunk by bytes, anchor validation
  -> context.rs     maintainer context (conventions/specs/skills) + auto-context
  -> agent.rs       build prompt, call LLM via provider.rs, parse JSON findings
  -> analysis.rs    ingest analyzer evidence (SARIF files, check-run annotations)
  -> quality.rs     apply policy: severity/confidence/allow-list filtering
  -> output.rs      summary, counts, cost/usage
  -> github.rs      post + reconcile inline review comments
```

## Execution state and reconciliation

Both modes use the same comment reconciliation:
`reconcile_review_comments` (github.rs) lists the PR's existing Cururu comments,
then updates, deletes, or creates so the PR converges to the desired set for
the current head SHA. A stale comment never needs a "please delete me" step —
the next run fixes it.

## Trust boundaries

- The **base commit** is trusted: `.cururu.toml`, context files, and the review
  prompt (`prompts/review.md`) come from there.
- The **PR head branch** is untrusted input: diff text is data, never executed
  and never trusted as instructions. Details in [SECURITY.md](../SECURITY.md).
- Credentials enter only through GitHub Actions secrets, never through
  repository files.
- The App server validates GitHub's HMAC-SHA256 signature before queueing any
  event. Installation tokens are short-lived and generated from the App private
  key at runtime; credentials are not stored in queue payloads or TOML.
- The durable queue stores bounded webhook payloads for idempotency and retry,
  then prunes completed/failed rows after seven days. Treat stored comment and
  PR text as untrusted input.

## Configuration layering

`src/config/` mirrors the domains it configures: `github` (repo/PR coordinates),
`provider` (LLM), `review` (diff limits, language, policy), `context`,
`summary`, `analysis`, plus `schema` (the `.cururu.toml` TOML shape) and `env`
(env-var helpers). Precedence, most to least specific:

1. Environment variables (workflow inputs)
2. Local `.cururu.toml` values (from trusted PR base), applied over shared base
3. Shared config from a pinned full commit SHA, when declared
4. Profile defaults (`balanced`, `strict`, `security`, `minimal`)

Local scalars override shared scalars. Review ignore patterns, focus rules,
context paths, and automatic-context inclusion/exclusion lists combine uniquely,
base first then local. Other arrays replace rather than append. `cururu init`
scaffolds a local config and workflow; `cururu print-config` can inspect the
effective config locally (a GitHub token is needed to read a shared source).
