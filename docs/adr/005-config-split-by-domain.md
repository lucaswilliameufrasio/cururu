# ADR 005: Config split by domain

- **Status:** Accepted
- **Date:** August 31, 2026
- **Decision makers:** Lucas Eufrasio
- **References:**
  - [The Rule of Three and domain-driven modularization]
  - `docs/architecture.md` (configuration layering)

## Context

`src/config.rs` grew to **1,167 lines** — the largest module in the project,
larger than the analysis pipeline it configures (`analysis.rs`, 475 lines). A
configuration module that outgrows the feature it configures has become a
product of its own, and this one carried five distinct concerns in one file:

- runtime config structs (`AppConfig`, `GitHubConfig`, `LlmConfig`,
  `ReviewConfig`, `ContextConfig`, `SummaryConfig`, `AnalysisConfig`)
- domain enums and policy parsing (`LlmProvider`, `CommentMode`, `FailOn`,
  `Severity`, profile presets)
- the `.cururu.toml` deserialization schema (7 TOML structs)
- environment-variable helpers and glob building
- ~435 lines of tests exercising env/TOML merge behavior

Every new configuration field (the `analysis` sections arrived in v4.2–v4.4)
extended a single file that reviewers of the actual review logic had to scroll
past, and the merge function (`merge_toml_str`) mixed all five concerns in one
190-line block.

## Decision

Split configuration into a `src/config/` module, one file per domain:

| Module | Contents |
|---|---|
| `mod.rs` | `AppConfig`, `from_env`, `merge_toml_str` orchestration, public re-exports |
| `github.rs` | `GitHubConfig` (repo/PR coordinates, API URLs) |
| `provider.rs` | `LlmProvider`, `LlmConfig` (LLM endpoint/model concerns) |
| `review.rs` | `ReviewConfig`, `ReviewPolicy`, `CommentMode`, `FailOn`, `Severity`, profile presets |
| `context.rs` | `ContextConfig`, `AutoContextConfig` |
| `summary.rs` | `SummaryConfig` |
| `analysis.rs` | `AnalysisConfig` |
| `schema.rs` | All `*Toml` deserialization structs (the `.cururu.toml` shape) |
| `env.rs` | `env_required`, `env_optional`, `env_parse`, `build_globs` |
| `tests.rs` | The existing 30 config tests, unchanged |

`mod.rs` re-exports the public types, so `use crate::config::{...}` in
`agent.rs`, `analysis.rs`, `context.rs`, `github.rs`, `main.rs`, `quality.rs`,
and `review.rs` is untouched.

## Consequences

### Positive

- **Proportion:** the largest config module is now `mod.rs` at 298 lines
  (orchestration) — the config surface is smaller than the pipeline it serves.
- **Domain navigation:** a change to provider behavior edits `provider.rs`, not
  a fifth of a monolith.
- **Zero caller churn:** re-exports preserve the external API; 63 tests stay
  green without modification.

### Negative / Trade-offs

- Ten files instead of one; importing a new field type requires touching one
  domain file plus `mod.rs`.
- TOML schema structs live in `schema.rs` rather than beside each domain
  config; accepted because the schema is a single versioned contract
  (`version = 1`) and must be read as a whole.

### Risks

- New contributors may add fields directly in `schema.rs` without wiring the
  merge in `mod.rs`; the merge function is the single integration point and is
  covered by the env-over-TOML precedence tests.

## Alternatives Considered

- **Keep the single file:** no churn, but the module keeps growing with every
  field and already dwarfed the domain logic.
- **Split tests per domain module too:** rejected for now — the tests exercise
  cross-domain behavior (env vars overriding TOML across provider/review/
  policy), and co-locating them in `tests.rs` documents the merge precedence in
  one place.
