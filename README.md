# Cururu

![cururu-github](https://the-counter.lucaswilliameufrasio.com/v1/badges/cururu-github?label=Visualiza%C3%A7%C3%B5es&label_color=%23555&color=%2350c700)

A stateless Rust PR review bot for GitHub Actions. Runs on any repository without
installation — add one workflow file and configure it with `.cururu.toml`.

```text
pull_request event
  -> GitHub Actions
  -> cururu action (Docker)
  -> GitHub API diff
  -> LLM review (OpenAI / OpenRouter / Groq)
  -> PR summary comment with usage
```

## Quick start

Add `.github/workflows/cururu-review.yml` to any repository:

```yaml
name: Cururu PR Review
on: pull_request_target
permissions:
  contents: read
  pull-requests: write
  issues: write
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: lucaswilliameufrasio/cururu@v4
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          llm_api_key: ${{ secrets.LLM_API_KEY }}
```

Set `LLM_API_KEY` as a repository secret. That is it.

## Configuration

Cururu reads `.cururu.toml` from the trusted base commit of the PR.

```toml
version = 1

[provider]
name = "openrouter"
model = "openai/gpt-5.6-luna"
temperature = 0.1
max_output_tokens = 4000

[review]
max_diff_bytes = 180000
chunk_bytes = 45000
ignore = ["**/*.lock", "dist/**"]
language = "pt-BR"
comment_mode = "inline"

[policy]
minimum_confidence = 0.65
max_findings = 30
fail_on = "off"
allowed_severities = ["critical", "high", "medium", "low"]
suggested_changes = false
incremental = false
synthesis = false
focus = []

[summary]
show_cost = true
show_usage = true

[context]
conventions = ["AGENTS.md", "CONTRIBUTING.md"]
specifications = ["docs/sdd/**/*.md", "docs/gdd/**/*.md"]
skills = [".agents/skills/**/SKILL.md"]
additional = ["docs/adr/**/*.md"]
max_bytes = 100000

[context.auto]
enabled = false
max_bytes = 50000
max_files = 20
per_file_bytes = 12000
include = ["src/**", "tests/**"]
exclude = ["**/generated/**", "**/*.min.js"]
```

### Provider

| `name` | Default model | Default base URL | Input/1M | Output/1M |
|---|---|---|---|---|
| `openrouter` **default** | `openai/gpt-5.6-luna` | `https://openrouter.ai/api/v1` | $1.00 | $6.00 |
| `openai` | `gpt-5.6-luna` | `https://api.openai.com/v1` | $1.00 | $6.00 |
| `groq` | `openai/gpt-oss-120b` | `https://api.groq.com/openai/v1` | $0.15 | $0.60 |

`base_url`, `model`, `temperature`, and `max_output_tokens` in TOML override the
provider defaults. Environment variables `LLM_BASE_URL`, `LLM_MODEL`,
`LLM_TEMPERATURE`, and `LLM_MAX_OUTPUT_TOKENS` override the corresponding TOML
values.

`temperature` controls how deterministic the review is. Keep it low (`0.0` to
`0.2`) for consistent, factual code reviews; increase it only when a project
prefers more varied suggestions. `max_output_tokens` limits the model response
per call. Use `2000` to `4000` for normal PRs, and increase it for large PRs or
when findings are being truncated. Larger values can increase cost.

### Context files

Context documents (conventions, specifications, skills) are loaded from the PR
base commit through the GitHub API and injected into the system prompt. The diff
is kept separate as untrusted input.

Set `max_bytes` to cap total context size. Files are loaded in order and
truncated if the combined content exceeds the limit.

Automatic source context is opt-in under `[context.auto]`. When enabled, Cururu
fetches only base-commit versions of changed files matching `include`, subject
to `max_files`, `max_bytes`, and `per_file_bytes`. Files matching `exclude` are
skipped. This keeps repository context trusted and bounded.

### Cost

A typical PR review uses ~3K tokens (small PR) to ~8K tokens (medium PR with
context files). Estimated cost per review through OpenRouter pricing:

| Model | Input/1M tok | Output/1M tok | Small PR (~$0.01) | Medium PR (~$0.03) |
|---|---|---|---|---|
| `openai/gpt-5.6-luna` **default** | $1.00 | $6.00 | ~$0.006 | ~$0.013 |
| `openai/gpt-oss-120b` (Groq) | $0.15 | $0.60 | ~$0.001 | ~$0.002 |
| `gemini-3.5-flash` | $2.00 | $9.00 | ~$0.010 | ~$0.023 |
| `gpt-5.6-terra` | $3.00 | $15.00 | ~$0.015 | ~$0.036 |
| `qwen/qwen3.6-27b` (Groq preview) | $0.60 | $3.00 | ~$0.003 | ~$0.007 |

**How cost reporting works:**

- OpenRouter returns per-request cost in the API response. When `show_cost =
  true` the total is shown in the summary.
- OpenAI and Groq do not return monetary cost per request. The summary will
  show token counts.
- All costs estimate through OpenRouter pricing. Direct provider pricing may
  differ.

### Review

| Field | Description |
|---|---|
| `max_diff_bytes` | Hard cap for reviewed diff size (default `180000`) |
| `chunk_bytes` | Chunk size before each LLM call (default `45000`) |
| `ignore` | Comma-separated glob patterns to skip in diff |
| `language` | Language for LLM-generated findings (default `pt-BR`) |
| `comment_mode` | `inline` (default) or `summary` |

`CURURU_LANGUAGE` environment variable overrides the TOML value.

The review prompt is intentionally language- and framework-agnostic. It
systematically considers security boundaries, authentication and authorization,
injection, secrets, error handling, concurrency, resource limits, external
calls, compatibility, migrations, observability, and tests. It only reports a
category when the changed code provides concrete evidence of a problem.

### Comment modes

Cururu can post review feedback in two ways, configured via `[review].
comment_mode`:

**`inline` (default)** — one review comment anchored to each finding's diff
line, like a normal human review. Comments carry the severity, finding, and
suggestion, with line-level highlights on the changed lines. On subsequent
pushes Cururu updates comments that remain relevant and removes those that are
no longer flagged, keeping the review in sync.

**`summary`** — a single compact comment in the PR conversation with a findings
table, tokens, and cost. This is the previous behavior; it updates in place via
a marker instead of duplicating.

The action requires `pull-requests: write` and `issues: write` permissions to
post inline comments and the summary comment respectively.

### Summary

| `show_cost` | Show provider-reported cost |
| `show_usage` | Show token counts (prompt, completion, cached, reasoning) |

### Policy and profiles

The optional `[policy]` section controls how findings are retained and whether
the Action fails. `fail_on = "off"` is the default and never blocks existing
consumers.

| Field | Default | Description |
|---|---:|---|
| `minimum_confidence` | `0.65` | Minimum confidence from the model (`0..=1`) |
| `max_findings` | `30` | Maximum findings posted |
| `fail_on` | `off` | `off`, `critical`, `high`, `medium`, or `low` |
| `allowed_severities` | all | Severities retained in the result |
| `suggested_changes` | `false` | Enable safe one-line GitHub suggestions |
| `incremental` | `false` | Enable incremental review state |
| `synthesis` | `false` | Enable cross-chunk synthesis |
| `focus` | `[]` | Review focus hints such as `security` or `tests` |

Built-in profiles can be selected with `review.profile`: `balanced` (default),
`strict`, `security`, or `minimal`. Explicit `[policy]` fields override the
selected profile.

## Environment variables

Secrets are always passed through GitHub Actions secrets / environment, never
through repository configuration.

| Variable | Required | Default | Description |
|---|---|---|---|
| `GITHUB_TOKEN` | yes | — | GitHub API token (automatic in Actions) |
| `LLM_API_KEY` | yes | — | LLM provider API key |
| `LLM_BASE_URL` | no | provider default | Override API base URL |
| `LLM_MODEL` | no | provider default | Override model name |
| `LLM_TEMPERATURE` | no | `0.1` | Override response randomness |
| `LLM_MAX_OUTPUT_TOKENS` | no | `4000` | Override maximum response tokens |
| `CURURU_PROVIDER` | no | `openrouter` | Override provider name |
| `CURURU_IGNORE` | no | lockfiles, dist, build | Comma-separated globs to skip in diff |
| `CURURU_MAX_DIFF_BYTES` | no | `180000` | Hard cap for reviewed diff size |
| `CURURU_CHUNK_BYTES` | no | `45000` | Chunk size before each LLM call |
| `CURURU_LANGUAGE` | no | `pt-BR` | Review language (overrides TOML) |
| `CURURU_PROFILE` | no | `balanced` | Review profile |
| `CURURU_FAIL_ON` | no | `off` | Fail the action at a severity threshold |

## Fork safety

The example workflow uses `pull_request_target` so the action runs in the
repository context, not the fork. Cururu reads the diff and context files
through the GitHub API and never executes code from the PR branch.

## Local development

```bash
export GITHUB_TOKEN=ghp_xxx
export GITHUB_REPOSITORY=owner/repo
export PR_NUMBER=123
export LLM_API_KEY=sk_xxx

cargo run -- print-diff
cargo run -- dry-run
cargo run -- review
cargo run -- print-config
```

## Comment commands

The example workflows also listen for `issue_comment`. Authorized repository
collaborators can request an explicit review with:

```text
/cururu review
/cururu review --full
```

Only exact commands are accepted. The commenter must have `write`, `maintain`,
or `admin` permission, and comment text cannot change the model, endpoint,
prompt, or secrets.

## Commands

```
cururu print-diff     Print the PR diff
cururu dry-run        Review and print JSON, do not post comment
cururu review         Review and post summary comment
cururu print-config   Print merged configuration
```

## Security

See `SECURITY.md`.
