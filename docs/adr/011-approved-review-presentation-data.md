# ADR 011: Approved review presentation preferences are user data

- **Status:** Accepted
- **Date:** 2026-10-19

## Context and decision

Desktop must support explicit repository language/tone/technical-level/detail
preferences without copying CLI defaults or promoting untrusted config into
system instructions. Add a bounded optional ReviewPreferences contract to engine
and an opt-in OpenAiCompatibleAgent builder that sends it as separate JSON user
data. Share existing choice lists with CLI validation; do not change CLI defaults,
environment precedence or its existing prompt assembly in this slice.

Desktop includes these preferences only after the same explicit local preview
and inclusion/diff-only decision as context. Bind them to approved evidence
fingerprints and fresh repository guards. Empty preferences preserve the existing
wire shape. They cannot change provider destinations/model/key, permissions,
publication mode, tool access or request count. Language is bounded text, not an
operating instruction or evidence that a repository document exists.

## Alternatives and consequences

Appending config values to system instructions would elevate untrusted input.
Encoding them as synthetic ContextFile entries would misrepresent source files.
A separate typed user-data contract preserves provenance and avoids both. No I/O
or runtime AppConfig assembly moves to engine. Revisit broader presentation
settings only with equally explicit authority and transmission boundaries.
