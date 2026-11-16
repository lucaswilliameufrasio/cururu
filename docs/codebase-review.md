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
- **At most 5000 files** are read; a larger selection fails and suggests
  `--path`, since empty and binary files cost a read without counting toward
  `max_bytes`.
- **Binary files and files Cururu cannot read as text** are listed as not
  reviewed, as in change reviews. The GitHub API returns content decoded with
  replacement characters, so a file containing U+FFFD is treated as binary
  there; legitimate text with that character is rare and is reported, not
  hidden.
- **Paths that cannot be written into a diff header safely** (a newline or
  other control character, or ` b/`) are left out and listed as not reviewed.

## Channels

| Channel | How | Output |
| --- | --- | --- |
| CLI | `cururu prepare-review --codebase [--path GLOB]...`, then `cururu api-review --codebase [--path GLOB]... --confirm-send [--format markdown]` | The JSON receipt, or with `--format markdown` a readable list of the problems found. Nothing is published; `publish-agent-review` refuses codebase receipts. |
| MCP | `prepare_review`, `validate_review` and `assemble_review` with `"source": "codebase"` and optional `"paths": [...]` | The prepared input for the host's own agent; nothing is published. |
| GitHub Action | `mode: codebase` with optional `codebase_paths`, run by `workflow_dispatch` and optionally a schedule; see [`docs/examples/cururu-codebase-review.yml`](examples/cururu-codebase-review.yml). The trusted configuration, files and context come from the reviewed commit (`GITHUB_SHA`, or `codebase_revision`). | An issue with the report, linking each finding to its file and line at that commit; `issue_url` output. With `publication.require_approval`, the report is shown in the job summary and the issue is opened only after the `cururu-publish` environment approves the publish job; a report that does not match its digest is refused. |
| GitHub App | Planned: `/cururu review codebase [paths]` in an issue | A reply in that issue. |

Hosted publication follows `[publication] require_approval`: directly by
default, or after a separate approval when it is on.

## Decisions

Recorded with the user on 2026-11-15: the Action posts the report as an issue;
MCP, CLI and other direct channels return the report as output; the Action runs
on demand and optionally on a schedule; the App answers a command in an issue;
the default scope is the whole repository within its own limit.
