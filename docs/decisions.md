# Architecture Decision Records (ADR)

## ADR 001: Docker action, API-only, no PR checkout
- Date: July 2026
- Context: Reviewing untrusted PR branches from Actions risks executing contributor code.
- Decision: Ship as a Docker action pinned by OCI digest; read diff, files, and metadata exclusively through the GitHub API. Never check out the PR.
- Consequences: No code-execution surface from PR content; the digest pin means a moved tag cannot swap the reviewed-in code.

## ADR 002: `pull_request_target` + base-commit trust boundary
- Date: July 2026
- Context: Configuration and context files must be maintainer-controlled, and the workflow needs secrets.
- Decision: Consumer workflows trigger on `pull_request_target`; `.cururu.toml` and all context files are read from the base commit, never the head branch.
- Consequences: Contributors cannot steer the review through their branch; forks get reviews without secret exposure.

## ADR 003: OpenAI-compatible multi-provider via `base_url`
- Date: July 2026
- Context: Locking into one LLM vendor couples a CI reviewer to one price/latency/availability profile.
- Decision: One OpenAI-compatible client; OpenAI, OpenRouter, and Groq selected by name with per-provider default base URL and model, overridable by `LLM_BASE_URL`/`LLM_MODEL`.
- Consequences: Adding a provider is a table entry; custom/self-hosted endpoints work but receive the API key, so they must be trusted (documented in SECURITY.md).

## ADR 004: Analyzer evidence via manifest + SARIF
- Date: August 2026
- Context: LLM-only reviews miss mechanically verifiable issues (types, lint, SAST).
- Decision: A trusted workflow publishes a JSON manifest describing tool runs; cururu ingests SARIF files and check-run annotations, rejects stale manifests (head-SHA mismatch), and filters findings to changed files.
- Consequences: Evidence status (`passed/partial/stale/no_evidence`) is explicit; PRs cannot forge evidence for code they did not touch.

## ADR 005: Config split by domain
- Date: August 31, 2026
- Context: `config.rs` reached 1,167 lines — larger than the analysis pipeline (475) — mixing provider, GitHub, review policy, context, and TOML schema concerns.
- Decision: Split into `src/config/` with one module per domain (`github`, `provider`, `review`, `context`, `summary`, `analysis`) plus `schema` (TOML shape), `env` (helpers), and `tests`; `mod.rs` keeps `AppConfig`, the merge orchestration, and public re-exports so no caller changes.
- Consequences: Largest config module drops to 298 lines; external API unchanged. See [docs/adr/005-config-split-by-domain.md](adr/005-config-split-by-domain.md).
