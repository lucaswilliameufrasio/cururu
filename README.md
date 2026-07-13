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
model = "openai/gpt-5-mini"

[review]
max_diff_bytes = 180000
chunk_bytes = 45000
ignore = ["**/*.lock", "dist/**"]

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

| `name` | Default model | Default base URL |
|---|---|---|
| `openai` | `gpt-5-mini` | `https://api.openai.com/v1` |
| `openrouter` | `openai/gpt-5-mini` | `https://openrouter.ai/api/v1` |
| `groq` | `llama-3.3-70b-versatile` | `https://api.groq.com/openai/v1` |

`base_url` and `model` in TOML override the provider defaults. Environment
variables `LLM_BASE_URL` and `LLM_MODEL` override all TOML values.

### Context files

Context documents (conventions, specifications, skills) are loaded from the PR
base commit through the GitHub API and injected into the system prompt. The diff
is kept separate as untrusted input.

Set `max_bytes` to cap total context size. Files are loaded in order and
truncated if the combined content exceeds the limit.

### Cost

- OpenRouter returns per-request cost in the API response. When `show_cost =
  true` the total is shown in the summary.
- OpenAI and Groq do not return monetary cost per request. The summary will
  show token counts.

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
| `CURURU_PROVIDER` | no | `openai` | Override provider name |
| `CURURU_IGNORE` | no | lockfiles, dist, build | Comma-separated globs to skip in diff |
| `CURURU_MAX_DIFF_BYTES` | no | `180000` | Hard cap for reviewed diff size |
| `CURURU_CHUNK_BYTES` | no | `45000` | Chunk size before each LLM call |

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
