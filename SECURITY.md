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

## Safe Action usage

For repositories accepting external PRs, use `pull_request_target` to avoid
checking out untrusted code:

```yaml
on: pull_request_target
```

Cururu reads the diff and context files through the GitHub API and never
executes code from the PR branch.
