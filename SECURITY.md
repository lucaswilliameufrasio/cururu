# Security Policy

## Reporting a Vulnerability

Open a GitHub issue on this repository for any security concerns. Do not disclose
sensitive details in public issues — email the maintainer directly if the issue
is sensitive.

## Trust model

- **Credentials** (`LLM_API_KEY`, `GITHUB_TOKEN`) are always supplied through
  GitHub Actions secrets, never through repository configuration.
- **Repository configuration** (`.cururu.toml`) is read from the PR's base
  commit, not from the contributor-controlled head branch.
- **The diff** is treated as untrusted input and sent to the configured LLM
  provider. Do not review PRs containing secrets.
- **Context files** (conventions, specifications, skills) are read from the base
  commit to prevent prompt injection through the contributor branch.
- **Automatic context**, when enabled, is also read only from the base commit and
  is bounded by file and byte limits.
- **Custom LLM endpoints** receive the configured API key; only enable
  `base_url` for an endpoint controlled and trusted by the repository owner.

## Safe Action usage

For repositories accepting external PRs, use `pull_request_target` to avoid
checking out untrusted code:

```yaml
on: pull_request_target
```

Cururu reads the diff and context files through the GitHub API and never
executes code from the PR branch.

Comment-triggered reviews use `issue_comment` and accept only exact Cururu
commands from users with repository write-level permission or higher. The
comment body is treated as data and cannot inject arbitrary prompts or shell
commands.
