# Architecture

> See also: [Glossary](glossary.md) for terms like anchor, base commit, and quality gate, and the [Security Policy](../SECURITY.md) for the trust model.

Cururu is a **stateless** GitHub Actions PR review bot. One Docker run reviews
one PR and exits; there is no database, cache, or background worker.

## Review pipeline

```
commands.rs (CLI: review)
  -> config/        env vars + .cururu.toml (base commit) merged into AppConfig
  -> github.rs      fetch PR diff, review comments, context files via GitHub API
  -> diff.rs        parse unified diff, chunk by bytes, anchor validation
  -> context.rs     maintainer context (conventions/specs/skills) + auto-context
  -> agent.rs       build prompt, call LLM via provider.rs, parse JSON findings
  -> analysis.rs    ingest analyzer evidence (SARIF files, check-run annotations)
  -> quality.rs     apply policy: severity/confidence/allow-list filtering
  -> output.rs      summary, counts, cost/usage
  -> github.rs      post + reconcile inline review comments
```

## Statelessness and reconciliation

Because every run is fresh, comment management is **reconciling**:
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

## Configuration layering

`src/config/` mirrors the domains it configures: `github` (repo/PR coordinates),
`provider` (LLM), `review` (diff limits, language, policy), `context`,
`summary`, `analysis`, plus `schema` (the `.cururu.toml` TOML shape) and `env`
(env-var helpers). Precedence, most to least specific:

1. Environment variables (workflow inputs)
2. `.cururu.toml` sections, applied field-by-field
3. Profile defaults (`balanced`, `strict`, `security`, `minimal`)
