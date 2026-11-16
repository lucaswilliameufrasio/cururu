# Codebase review

Cururu reviews changes by default. A codebase review reads the **committed
files at one revision** as existing code instead, for example to audit an old
project. Uncommitted and untracked files are never part of it.

## How it works

The selected files are shown to the regular review pipeline as a diff that
adds every file to an empty tree. Chunking, line anchors, validation, retries,
the split of large files, binary-file reporting and repository context are
therefore the same as for a change, and every line can carry a finding. The
prompt adds a codebase mode (`prompts/codebase.md`): there is no regression to
report, nothing is "added", and a problem must not depend on files outside the
selection.

- **Selection:** every tracked file, or the files matching `--path` globs
  (repeatable), minus `review.ignore`.
- **Limit:** `[codebase] max_bytes` (default 1000000) bounds the collected file
  content. A larger selection fails before anything is sent and suggests
  narrowing it with `--path`. `prepare-review --codebase` shows the files and
  chunks, so the size can be checked before `--confirm-send`.
- **Binary files and files Cururu cannot read as text** are listed as not
  reviewed, as in change reviews.

## Channels

| Channel | How | Output |
| --- | --- | --- |
| CLI | `cururu prepare-review --codebase [--path GLOB]...`, then `cururu api-review --codebase [--path GLOB]... --confirm-send [--format markdown]` | The JSON receipt, or with `--format markdown` a readable list of the problems found. Nothing is published; `publish-agent-review` refuses codebase receipts. |
| MCP | `prepare_review`, `validate_review` and `assemble_review` with `"source": "codebase"` and optional `"paths": [...]` | The prepared input for the host's own agent; nothing is published. |
| GitHub Action | Planned: `workflow_dispatch` and an optional schedule | An issue with the report. |
| GitHub App | Planned: `/cururu review codebase [paths]` in an issue | A reply in that issue. |

Hosted publication follows `[publication] require_approval`: directly by
default, or after a separate approval when it is on.

## Decisions

Recorded with the user on 2026-11-15: the Action posts the report as an issue;
MCP, CLI and other direct channels return the report as output; the Action runs
on demand and optionally on a schedule; the App answers a command in an issue;
the default scope is the whole repository within its own limit.
