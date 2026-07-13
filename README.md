# pullfrog-rs

A stateless Rust PR review bot for GitHub Actions, inspired by Pullfrog.

It runs as a CLI inside GitHub Actions, fetches the pull request diff, sends it to an LLM, and upserts one summary comment on the PR.

## Why stateless?

The GitHub PR is the state. This project does not need a database for the MVP:

- PR metadata comes from GitHub events/API
- the diff comes from the GitHub API
- the review result is written back as a PR comment
- duplicate bot comments are avoided with a hidden marker

Add PostgreSQL later only for SaaS features like org settings, analytics, historical memory, feedback, or cost tracking.

## Current design

```text
pull_request event
  -> GitHub Actions
  -> pullfrog-rs CLI
  -> GitHub REST API diff
  -> diff parser/filter/chunker
  -> LLM review agent
  -> PR summary comment
```

The default transport is OpenAI-compatible HTTP, so it works with OpenAI, OpenRouter, Groq-compatible endpoints, and similar APIs.

Rig support is prepared as a feature flag:

```bash
cargo build --features rig
```

The `RigAgent` adapter is intentionally thin for now. I would keep the first production version OpenAI-compatible and swap the internals later if you want Rig-native tools, structured output, embeddings, or multi-provider orchestration.

## Requirements

- Rust `1.96.1`
- GitHub Actions `GITHUB_TOKEN`
- `LLM_API_KEY`

## Local usage

```bash
export GITHUB_TOKEN=ghp_xxx
export GITHUB_REPOSITORY=owner/repo
export PR_NUMBER=123
export LLM_API_KEY=sk_xxx
export LLM_BASE_URL=https://api.openai.com/v1
export LLM_MODEL=gpt-5-mini

cargo run -- print-diff
cargo run -- dry-run
cargo run -- review
```

For OpenRouter-style usage:

```bash
export LLM_BASE_URL=https://openrouter.ai/api/v1
export LLM_MODEL=openai/gpt-5-mini
```

## Environment variables

| Variable | Required | Default | Description |
|---|---:|---|---|
| `GITHUB_TOKEN` | yes | - | Token used to read PRs and write comments |
| `GITHUB_REPOSITORY` | yes | - | `owner/repo` |
| `PR_NUMBER` | yes | - | Pull request number |
| `LLM_API_KEY` | yes | - | LLM API key |
| `LLM_BASE_URL` | no | `https://api.openai.com/v1` | OpenAI-compatible API base URL |
| `LLM_MODEL` | no | `gpt-5-mini` | Model name |
| `PULLFROG_PROVIDER` | no | `openai-compatible` | `openai-compatible` or `rig` |
| `PULLFROG_SUMMARY_ONLY` | no | `true` | Summary comment mode |
| `PULLFROG_MAX_DIFF_BYTES` | no | `180000` | Hard cap for reviewed diff size |
| `PULLFROG_CHUNK_BYTES` | no | `45000` | Chunk size before LLM call |
| `PULLFROG_IGNORE` | no | lockfiles/build outputs | Comma-separated glob patterns |

## Security notes

Use `pull_request`, not `pull_request_target`, by default. Keep permissions small:

```yaml
permissions:
  contents: read
  pull-requests: read
  issues: write
```

This bot writes a PR timeline comment through the Issues comments API because every PR is also an issue.

## Commands

```bash
pullfrog-rs print-diff
pullfrog-rs dry-run
pullfrog-rs review
```

## Next improvements

- Inline comments using `line`/`side`, not deprecated `position`
- SARIF output for GitHub code scanning
- provider-specific adapters
- repo policy file, e.g. `.pullfrog.yml`
- prompt/version hash in the comment
- optional Check Run output
