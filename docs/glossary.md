# Review & Architecture Glossary

Terms used across the codebase, the docs, and the review pipeline.

---

## Prompt Injection

An attack where untrusted text tries to override the LLM's instructions by being
embedded in the input (e.g., a diff saying "ignore previous instructions").

**Mitigation in cururu:** the prompt is a versioned file (`prompts/review.md`)
that treats the diff as data and demands JSON output; findings must pass anchor
validation; the bot never executes PR code and never places credentials in the
prompt. Full model in [SECURITY.md](../SECURITY.md).

---

## Base commit vs head branch

The **base commit** is the PR's target state (the branch being merged into) and
is maintainer-controlled. The **head branch** belongs to the contributor and is
untrusted.

**Use in cururu:** `.cururu.toml`, conventions, specifications, and skills are
read from the base commit only, so a contributor cannot steer the review by
adding files to their branch.

---

## `pull_request_target` vs `pull_request`

GitHub Actions trigger events. `pull_request_target` runs the workflow from the
**base** branch with access to secrets; `pull_request` runs the workflow from
the **merge commit** of the PR with no secrets.

**Use in cururu:** consumer workflows use `pull_request_target` so the review
runs trusted code while reading untrusted PR content through the API.

---

## Anchor

A `(path, line)` pair where an inline review comment can legally be attached.

**Validation:** `is_valid_anchor` (diff.rs) rejects findings whose path is not
in the diff or whose line is outside a changed hunk. The LLM cannot make cururu
comment on arbitrary files.

---

## Chunking

Splitting large diffs into `chunk_bytes`-sized pieces so each LLM request fits
the model context, bounded by `max_diff_bytes` for the whole PR.

---

## Reconciliation

Bringing the PR's existing Cururu review comments to the exact set desired for
the current head SHA: stale comments are deleted, changed ones are updated,
missing ones are created.

---

## Quality gate

The policy pass applied to raw LLM findings: drop severities outside
`allowed_severities`, drop findings below `minimum_confidence`, cap at
`max_findings`. The action's outputs (`quality_gate`, `fail_on` comparison)
are computed from the structured JSON result, never from free text.

---

## `fail_on`

The severity threshold that makes the action exit non-zero (`off` by default).
Ranks critical > high > medium > low; `off` never fails.

---

## Analysis manifest

A JSON file published by a trusted workflow describing which analyzer tools ran
and where their SARIF reports are. Cururu marks evidence `stale` when the
manifest does not match the PR head SHA (`require_current_head`) and filters
every SARIF finding to files changed in the diff.

---

## SARIF

Static Analysis Results Interchange Format — the standard JSON format for
linter/analyzer findings, used by the `[analysis]` feature as evidence alongside
LLM findings.

---

## Comment mode

`inline` (default) posts findings as PR review comments anchored to lines;
`summary` posts a single overview comment.
