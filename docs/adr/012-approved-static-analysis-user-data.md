# ADR 012: Approved static-analysis evidence is separate user data

- **Status:** Accepted
- **Date:** 2026-10-19

## Decision

Add an opt-in engine adapter builder for an already-approved normalized
AnalysisReport. Validate bounded count/bytes/path/severity/confidence and canonical
report status, then send JSON as a separate user message, never system
instructions or proof of tool execution. Empty evidence preserves the old wire
shape. This does not read artifacts, execute analyzers or authorize transmission.

Desktop must retain local preview as the default and require a distinct include
action before provider transmission. Bind normalized report and artifact hashes
through pre/postflight, archive, incremental lookup and suggestion guards. Diff-
only revokes this inclusion too. Merge original tool candidates through the
existing core collector rather than relying on the model to repeat diagnostics;
preserve origin/rule and final-policy suppressions. No raw SARIF, unrecognized
artifact fields, commands or credentials are added to the request.

## Consequences

Use core status/conversion/candidate rules rather than duplicate them. Hosts own
consent, final budgets, file/revision freshness and data collection. Diagnostic
prose may contain private content and repeat per bounded chunk; preview must warn
about that. Static-only archives remain distinct from provider reviews carrying
approved static evidence, and cannot satisfy provider incremental lookup.
