# Cururu

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
on: pull_request_target
permissions:
  contents: read
  pull-requests: read
  issues: write
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: lucaswilliameufrasio/cururu@v1
        with:
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

[review]
max_diff_bytes = 180000
chunk_bytes = 45000
ignore = ["**/*.lock", "dist/**"]
language = "pt-BR"

[summary]
show_cost = true
show_usage = true

[context]
conventions = ["AGENTS.md", "CONTRIBUTING.md"]
specifications = ["docs/sdd/**/*.md", "docs/gdd/**/*.md"]
skills = [".agents/skills/**/SKILL.md"]
additional = ["docs/adr/**/*.md"]
max_bytes = 100000

[summary]
show_cost = true
show_usage = true
```

### Provider

| `name` | Default model | Default base URL | Input/1M | Output/1M |
|---|---|---|---|---|
| `openrouter` **default** | `openai/gpt-5.6-luna` | `https://openrouter.ai/api/v1` | $1.00 | $6.00 |
| `openai` | `gpt-5.6-luna` | `https://api.openai.com/v1` | $1.00 | $6.00 |
| `groq` | `openai/gpt-oss-120b` | `https://api.groq.com/openai/v1` | $0.15 | $0.60 |

`base_url` and `model` in TOML override the provider defaults. Environment
variables `LLM_BASE_URL` and `LLM_MODEL` override all TOML values.

### Context files

Context documents (conventions, specifications, skills) are loaded from the PR
base commit through the GitHub API and injected into the system prompt. The diff
is kept separate as untrusted input.

Set `max_bytes` to cap total context size. Files are loaded in order and
truncated if the combined content exceeds the limit.

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

`CURURU_LANGUAGE` environment variable overrides the TOML value.

### Summary

| `show_cost` | Show provider-reported cost |
| `show_usage` | Show token counts (prompt, completion, cached, reasoning) |

## Environment variables

Secrets are always passed through GitHub Actions secrets / environment, never
through repository configuration.

| Variable | Required | Default | Description |
|---|---|---|---|
| `GITHUB_TOKEN` | yes | — | GitHub API token (automatic in Actions) |
| `LLM_API_KEY` | yes | — | LLM provider API key |
| `LLM_BASE_URL` | no | provider default | Override API base URL |
| `LLM_MODEL` | no | provider default | Override model name |
| `CURURU_PROVIDER` | no | `openrouter` | Override provider name |
| `CURURU_IGNORE` | no | lockfiles, dist, build | Comma-separated globs to skip in diff |
| `CURURU_MAX_DIFF_BYTES` | no | `180000` | Hard cap for reviewed diff size |
| `CURURU_CHUNK_BYTES` | no | `45000` | Chunk size before each LLM call |
| `CURURU_LANGUAGE` | no | `pt-BR` | Review language (overrides TOML) |

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

## Commands

```
cururu print-diff     Print the PR diff
cururu dry-run        Review and print JSON, do not post comment
cururu review         Review and post summary comment
cururu print-config   Print merged configuration
```

## Security

See `SECURITY.md`.
