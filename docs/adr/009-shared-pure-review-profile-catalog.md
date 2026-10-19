# ADR 009: Share the pure review profile catalog

- **Status:** Accepted
- **Date:** 2026-10-19
- **Decision makers:** Lucas Eufrasio

## Context and decision

The user explicitly approved sharing only review profile defaults/catalog after
earlier decisions had kept profiles in hosts. Move the existing pure ReviewPolicy
value/defaults and profile_defaults function to cururu-engine::review_policy.
CLI reexports these unchanged. Preserve balanced/strict/security/minimal values,
case/whitespace matching, original profile label and error text.

Environment precedence, AppConfig/runtime assembly, file/network access,
credentials, consent and publication remain hosts. The catalog does not authorize
any I/O. Desktop applies a profile only when explicitly declared, followed by
explicit repository overrides and its own safety ceilings. Without a profile,
its current defaults remain unchanged. Host support for incremental analysis or
other behavior is not implied by the presence of a catalog field.

## Alternatives and consequences

Duplicating profile constants would drift; retaining unsupported profiles would
leave common repository configuration unusable in desktop. Moving runtime merge
would exceed the approved scope. Share only pure values now; revisit separately
if another runtime rule needs a common contract. No provider or permission
defaults become repository-controlled as a result of this extraction.
