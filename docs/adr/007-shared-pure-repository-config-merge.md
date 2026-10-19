# ADR 007: Share pure repository TOML composition

- **Status:** Accepted
- **Date:** 2026-10-19
- **Decision makers:** Lucas Eufrasio

## Context and decision

Desktop shared-base support must not copy the CLI's existing repository merge
rule. The user explicitly approved extending ADR 006 to move only the pure TOML
document combination into `cururu-engine::config::compose_toml`. The CLI keeps
its compatibility method and delegates to this shared implementation.

Preserve existing behavior: tables merge recursively, local scalars/ordinary
arrays replace base values, and the established cumulative arrays append unique
local entries in order (`review.ignore`, `policy.focus`, context declarations,
and auto-context include/exclude). Preserve the existing parse/error messages
and unknown TOML fields; composition does not apply schema or host policy.

Environment precedence, profile expansion, AppConfig construction, secrets,
remote fetching, filesystem access, permissions and consent remain in hosts.
Composition cannot authorize a provider, file read, command or transmission.

## Alternatives and consequences

Duplicating merge code in desktop risks divergent semantics. Moving AppConfig
or runtime merging into the engine would violate the approved boundary. Sharing
only document composition avoids both. Hosts must still validate source schemas
and enforce size, provenance and permission limits; the pure function is not an
untrusted-input admission boundary. Revisit if another pure domain rule needs a
shared contract, rather than silently expanding engine ownership.
