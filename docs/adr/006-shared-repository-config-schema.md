# ADR 006: Share the portable repository configuration schema

- **Status:** Accepted
- **Date:** 2026-10-16
- **Decision makers:** Lucas Eufrasio

## Context

Desktop and CLI consume `.cururu.toml`, but the private CLI schema would force
the desktop to copy it or import CLI environment, credential and SCM behavior.
The user selected a shared schema/validation boundary rather than deferring the
integration. This supersedes ADR 005 only as to ownership of `schema.rs`.

## Decision

Expose the existing optional TOML shape, version-aware parser with path-aware
errors, and portable confidence/choice validation in `cururu-engine::config`.
Keep runtime configuration, environment precedence, profile presets, filesystem
reads, shared-base fetching, credentials, SCM, and authorization in each host.
The CLI's existing merge order and errors are preserved; unknown fields remain
ignored as before. Parsing configuration is not permission to execute or send.
In particular, parsing an evaluator section does not activate filtering.

`cururu-core` remains domain-only; no TOML or configuration loader is added to it.
The desktop must intersect repository restrictions with local limits/consent,
never adopt provider destinations or broaden permissions from repository data.

## Consequences and alternatives

The engine gains `toml` and `serde_path_to_error` (already CLI dependencies),
and the CLI retains a compatibility re-export for its merge code. Duplicating
the schema risks silent drift; moving all `AppConfig` would mix secrets, env,
I/O and host policy into the reusable engine. Neither was chosen. Revisit if
configuration validation needs host-independent typed policy resolution; do not
silently move environment or permission semantics with it.
