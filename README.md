# Cururu

<p align="center"><img src="assets/cururu-logo.svg" alt="Cururu" width="168"></p>

![cururu-github](https://the-counter.lucaswilliameufrasio.com/v1/badges/cururu-github?label=Visualiza%C3%A7%C3%B5es&label_color=%23555&color=%2350c700)

A self-service Rust code reviewer for GitHub pull requests. Use the Action or CLI,
or run the installable GitHub App yourself. Each repository controls its policy,
model and context; secrets remain with the Action owner or App operator.

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
  checks: read
  pull-requests: write
  issues: write
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - name: Check LLM API key
        id: llm_key
        env:
          LLM_API_KEY: ${{ secrets.LLM_API_KEY }}
        run: |
          if [ -z "$LLM_API_KEY" ]; then
            echo "skip=true" >> "$GITHUB_OUTPUT"
            echo "::notice::LLM_API_KEY is not configured — skipping Cururu review."
          else
            echo "skip=false" >> "$GITHUB_OUTPUT"
          fi
      - name: Review PR
        if: steps.llm_key.outputs.skip == 'false'
        uses: lucaswilliameufrasio/cururu@ee65b0ef0211a9a79ce14a2af597b45964ca7b8f # v4.6.0
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          llm_api_key: ${{ secrets.LLM_API_KEY }}
          cururu_language: pt-BR
          cururu_profile: balanced
          cururu_fail_on: off
```

Set `LLM_API_KEY` as a repository secret. That is it — if the secret is not
configured, the review is skipped with a notice instead of failing the PR.

## Integration discovery prompt

Before choosing a model or enabling Cururu in a production repository, use the
canonical [integration prompt](prompts/integration.md). It makes budget discovery a
mandatory first step, then reviews repository documentation and historical GitHub
activity before recommending a configuration.

The prompt is GitHub-specific. It optionally recommends the GitHub CLI (`gh`) so the
project owner can provide bounded metadata about commits, merged pull requests, PR
frequency, and PR size. `gh` is not required to run Cururu, but without it (or
equivalent GitHub API data) cost and historical-regression estimates are incomplete
and less reliable. The prompt must distinguish observed history from assumptions and
must never expose credentials.

The current review Action is a Docker Action. Installing `gh` on the GitHub Actions
runner does not automatically make it available inside Cururu's container. Use `gh`
during integration discovery, or add a separate data-producing workflow only after
designing a trusted input path for that data.

## Configuration

Cururu reads `.cururu.toml` from the trusted base commit of the PR.

The repository's [example `.cururu.toml`](.cururu.toml) is fully commented in English.
Its bounded diff, chunk, context, output, and finding limits make the cost and review
trade-offs explicit. Copy the relevant sections to a consuming repository and adjust
them after measuring real PR volume and risk.

```toml
version = 1

[provider]
name = "openrouter"
model = "openai/gpt-6-luna"
temperature = 0.1
max_output_tokens = 4000

[review]
max_diff_bytes = 180000
chunk_bytes = 45000
ignore = ["**/*.lock", "dist/**"]
language = "pt-BR"
tone = "neutral"
technical_level = "intermediate"
suggestion_detail = "detailed"
comment_mode = "inline"

[policy]
minimum_confidence = 0.65
max_findings = 30
fail_on = "off"
allowed_severities = ["critical", "high", "medium", "low"]
suggested_changes = false
incremental = false
synthesis = true
focus = ["correctness", "security", "performance", "cost", "regressions", "tests"]

[summary]
show_cost = true
show_usage = true
# Optional: public HTTPS image URL; shown only in the main PR summary comment.
# logo_url = "https://raw.githubusercontent.com/lucaswilliameufrasio/cururu/main/assets/cururu-logo.svg"

[context]
conventions = ["AGENTS.md", "CONTRIBUTING.md"]
specifications = ["docs/sdd/**/*.md", "docs/gdd/**/*.md"]
skills = [".agents/skills/**/SKILL.md"]
additional = ["docs/adr/**/*.md", "docs/architecture.md", "docs/decisions.md", "SECURITY.md", "CHANGELOG.md"]
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
| `openrouter` **default** | `openai/gpt-6-luna` | `https://openrouter.ai/api/v1` | See provider pricing | See provider pricing |
| `openai` | `gpt-6-luna` | `https://api.openai.com/v1` | See provider pricing | See provider pricing |
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

### Shared configuration

A repository can extend a shared configuration stored in another repository,
including a private one:

```toml
version = 1

[config]
base = "acme/engineering-standards"
base_ref = "0123456789abcdef0123456789abcdef01234567" # full commit SHA
base_path = "configs/cururu/base.toml"

[review]
language = "pt-BR"

[policy]
focus = ["payments", "domain rules"]
```

The local `.cururu.toml` is read from the trusted base commit of the pull
request. The shared source is selected only by that trusted file and must use a
full commit SHA; the shared file cannot recursively select another base.
Scalars in the local file override the base. Review ignore patterns, policy
focus, context paths, and automatic-context include/exclude patterns append
unique local entries after base entries. Other arrays, including allowed
severities and analyzer paths, replace the base array. The merged TOML is
validated before Cururu applies it.

For private bases, the token supplied as `github_token` must be able to read
the consuming repository and write its PR comments. When the base is in a
different installation, pass its read-only token as `shared_config_token`. A
GitHub App installation token with `contents: read` on the base is suitable. An
installation granted access to all repositories in an organization can read
new base repositories in that scope without changing the installation; a
selective installation must have each base repository added by an administrator.
Cross-organization repositories require access granted by that organization.
Never put tokens or other credentials in TOML.

For a base in a separate organization, the Action accepts a second,
read-only-scoped `shared_config_token`. The pinned example workflow
[`cururu-review-private-base.yml`](docs/examples/cururu-review-private-base.yml)
creates separate GitHub App installation tokens for the consumer and base.

### Cost

A typical PR review uses ~3K tokens (small PR) to ~8K tokens (medium PR with
context files). Estimated cost per review through OpenRouter pricing:

| Model | Input/1M tok | Output/1M tok | Small PR (~$0.01) | Medium PR (~$0.03) |
|---|---|---|---|---|
| `openai/gpt-6-luna` **default** | See provider pricing | See provider pricing | See provider pricing | See provider pricing |
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
| `tone` | `neutral` (default), `direct`, or `didactic` |
| `technical_level` | Intended reader: `beginner`, `intermediate` (default), or `expert` |
| `suggestion_detail` | Recommendation context: `concise`, `standard`, or `detailed` (default) |
| `comment_mode` | `inline` (default) or `summary` |

`suggestion_detail = "detailed"` asks Cururu to explain the context, why a fix
addresses the finding, and a concrete safe action. It should acknowledge when the
diff does not provide enough information for a reliable fix instead of guessing.
Tone, audience level, and suggestion detail are independent settings.

`CURURU_LANGUAGE` environment variable overrides the TOML value.

The review prompt is intentionally language- and framework-agnostic. It
systematically considers security boundaries, authentication and authorization,
injection, secrets, error handling, concurrency, resource limits, external
calls, compatibility, migrations, observability, and tests. It only reports a
category when the changed code provides concrete evidence of a problem.

### Comment modes

Cururu can post review feedback in two ways, configured via `[review].
comment_mode`:

Set `[summary].logo_url` to an absolute HTTPS URL to show the Cururu logo in the
main summary comment's signature, in place of the ASCII frog. It is omitted by
default and never added to inline finding comments. Set `logo_url = ""` locally
to disable a URL inherited from a shared base.

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

### Analyzer evidence

Cururu does not guess or execute a project's linter. Projects choose and run
their own tools in CI, then optionally provide SARIF evidence:

```toml
[analysis]
enabled = true
manifest = "artifacts/analysis.json"
sarif_paths = ["artifacts/**/*.sarif"]
max_findings = 100
require_current_head = true
```

SARIF findings are normalized into the same review format, limited to changed
files, deduplicated with LLM findings when synthesis is enabled, and posted in
the same comments. This works with compiler diagnostics, linters, security
scanners, and custom analyzers without Cururu knowing the project's technology.
Cururu only reads the files; it never executes commands from the repository.

The optional manifest records tool lifecycle separately from diagnostics:

```json
{
  "schema_version": 1,
  "commit_sha": "<head sha>",
  "tools": [
    {
      "name": "cargo-clippy",
      "status": "failed",
      "exit_code": 101,
      "message": "compilation failed",
      "sarif_path": "artifacts/clippy.sarif"
    }
  ]
}
```

Supported statuses are `passed`, `failed`, `not_run`, `skipped`, and
`timed_out`. A stale `commit_sha` is rejected when `require_current_head` is
enabled, preventing evidence from another PR revision from being presented as
current.

Cururu can also ingest analyzer evidence reported through GitHub **Check Runs**
annotations instead of a SARIF artifact:

```toml
[analysis]
enabled = true
check_runs = true
check_run_names = ["clippy", "rust-clippy"]
```

`check_run_names` is optional; when empty, annotations from all non-successful
check runs on the head commit are used. This requires the workflow token to
have `checks: read` permission:

```yaml
permissions:
  contents: read
  checks: read
```

Annotations are normalized to the same review format, filtered to changed
files, and deduplicated with LLM findings when synthesis is enabled.

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

## Self-hosted GitHub App

Cururu can also run as a webhook service you operate. It supports PostgreSQL or
SQLite for its durable, deduplicated webhook queue. The deployment guide covers
GitHub App permissions/events, private shared-config access, Compose setup,
SQLite online backups and optional Litestream replication:

**[Self-host the Cururu GitHub App](docs/deployment/github-app.md)**

Every operator creates a separate GitHub App whose webhook points to that
operator's Cururu deployment. The repository does not provide a shared App or
central webhook endpoint.

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

### Using the CLI in a consuming repository

Starting with the first release after the CLI distribution workflow is merged,
install the prebuilt CLI from the latest GitHub Release:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/lucaswilliameufrasio/cururu/releases/latest/download/cururu-installer.sh | sh
```

On Windows, run the generated PowerShell installer:

```powershell
irm https://github.com/lucaswilliameufrasio/cururu/releases/latest/download/cururu-installer.ps1 | iex
```

Each new release includes per-platform archives and SHA-256 checksums. The
current latest release predates these assets; until the next version is
published, build from source with Rust/Cargo installed:

```bash
cargo install --git https://github.com/lucaswilliameufrasio/cururu
cururu init
```

`cururu init` creates the starter `.cururu.toml` and
`.github/workflows/cururu-review.yml`. It will stop rather than overwrite either
file. Add `LLM_API_KEY` in the repository's Actions secrets and review the
workflow permissions before enabling it.

| Command | Purpose | Required context |
|---|---|---|
| `cururu init` | Scaffold the config and Action workflow | Run in the consumer repository |
| `cururu print-config` | Print merged local config without secrets | Local `.cururu.toml`; `GITHUB_TOKEN` or `CURURU_SHARED_CONFIG_TOKEN` for a shared remote base |
| `cururu print-diff` | Print the current PR diff | `GITHUB_TOKEN`, `GITHUB_REPOSITORY`, `PR_NUMBER` |
| `cururu dry-run` | Review a PR and print JSON without posting comments | GitHub PR context plus `LLM_API_KEY` |
| `cururu review` | Review a PR and post/update comments | GitHub PR context plus `LLM_API_KEY` and write permissions |
| `cururu serve` | Run the self-hosted GitHub App webhook service | GitHub App settings, `LLM_API_KEY`, and `CURURU_DATABASE_URL` |
| `cururu backup-sqlite <path>` | Make an online SQLite backup | SQLite `CURURU_DATABASE_URL` and a new destination path |

## Comment commands

The example workflows also listen for `issue_comment`. Authorized repository
collaborators can request an explicit review with:

```text
/cururu review
/cururu review --full
```

Automatic reviews run for `opened`, `synchronize`, `reopened`, and
`ready_for_review`. Draft PRs are intentionally skipped during draft pushes and
reviewed when changed to ready; authorized collaborators can still request a
manual review with `/cururu review`.

Only exact commands are accepted. The commenter must have `write`, `maintain`,
or `admin` permission, and comment text cannot change the model, endpoint,
prompt, or secrets.

## Commands

```
cururu init           Add a starter .cururu.toml and GitHub Actions workflow
cururu print-diff     Print the PR diff
cururu dry-run        Review and print JSON, do not post comment
cururu review         Review and post summary comment
cururu print-config   Print merged configuration
```

Run `cururu init` in a repository to create `.cururu.toml` and
`.github/workflows/cururu-review.yml`. It refuses to overwrite either file.
Review the generated files and add `LLM_API_KEY` as a repository secret before
enabling the workflow. `cururu print-config` can inspect the local TOML without
GitHub Actions or an LLM key; set `GITHUB_TOKEN` when the config references a
private shared base.

## Security

See `SECURITY.md`.
