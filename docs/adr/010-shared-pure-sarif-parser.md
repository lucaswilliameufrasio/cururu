# ADR 010: Share the pure SARIF parser

- **Status:** Accepted
- **Date:** 2026-10-19
- **Decision makers:** Lucas Eufrasio

## Context and decision

The user approved sharing only the CLI's existing SARIF JSON types/parsing and
pure finding conversion in cururu-engine, avoiding a second desktop parser.
Core retains domain conversion/normalization, not JSON parsing or I/O. The CLI
delegates while retaining file discovery, reads, error context, aggregate limits
and execution. Preserve first-location handling, defaults, changed-path filtering,
ordering and the legacy first-finding behavior when max_findings is zero.

This parser is not a safety/admission boundary: hosts must bound raw input,
artifact count, findings, paths and revision. Desktop requires explicit artifact
discovery/load/preview and fresh local guards. Parsing cannot fetch files, run
tools, send provider evidence, grant consent or publish. More restrictive desktop
admission must not silently alter CLI compatibility.

## Alternatives and consequences

Copying private CLI DTOs and traversal risks drift; moving file reads/execution
to the engine exceeds the approved scope. Share a pure parser now and keep host
behavior separate. Revisit richer SARIF features with compatibility tests rather
than assuming unsupported schema fields are authoritative.
