# ADR 008: Share pure automatic-context defaults and candidate selection

- **Status:** Accepted
- **Date:** 2026-10-19
- **Decision makers:** Lucas Eufrasio

## Context and decision

The user selected “Compartilhar regras puras” when asked how desktop should
reuse CLI automatic-context behavior without copying defaults or selection.
Move `AutoContextConfig` defaults and the pure include/exclude, duplicate and
capacity predicate to `cururu-engine::auto_context`. CLI retains a compatibility
re-export and its existing fetch/iteration behavior. Compile patterns once and
pass current occupied paths, count and bytes to the predicate after each read;
a failed/empty fetch must not consume a candidate slot.

This contract does not read files, choose a revision, fetch content, truncate
text, mutate stores, expand runtime profiles or grant consent. Hosts still own
I/O, final limits and credentials. In desktop, automatic context is only a
proposal of HEAD/base candidates: discover names, load a preview and explicitly
include it before transmission. Configuration cannot initiate these actions.

## Alternatives and consequences

Duplicating defaults in desktop would cause drift; requiring all parameters
explicitly would avoid duplication at the cost of incompatibility with existing
configuration. Moving fetch or runtime assembly to engine exceeds the approved
scope. Sharing the pure predicate preserves CLI ordering and dynamic budgets;
hosts may impose stricter bounds and reject excess rather than silently omit.
Revisit if selection needs revision-dependent information; do not add I/O to
this module to resolve it.
