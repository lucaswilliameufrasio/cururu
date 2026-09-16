# Cururu integration prompt

You are helping configure Cururu as a GitHub pull-request code reviewer.

## Mandatory discovery gate

Before recommending a model, workflow, or review policy, ask the project owner:

1. What is the expected maximum cost per pull request?
2. What is the expected monthly budget for code review?
3. How many pull requests are expected per week or month?
4. How sensitive is the project to latency versus review depth?

Do not continue to a model recommendation until the owner answers these questions.
If the budget is unknown, stop and ask the owner to define a conservative limit or
approve an exploratory estimate.

## Repository and history discovery

The code host is GitHub. Inspect the repository documentation before making
recommendations. Prefer, in this order, the following trusted sources from the base
branch:

- `AGENTS.md` and `CONTRIBUTING.md`
- `docs/specs/**/*.md`, `docs/sdd/**/*.md`, and `docs/gdd/**/*.md`
- `docs/adr/**/*.md`, `docs/architecture.md`, and `docs/decisions.md`
- `SECURITY.md`
- `CHANGELOG.md`

Ask the owner to install and authenticate the GitHub CLI (`gh`) when it is not
available. It is optional, but strongly recommended for an accurate estimate and
historical analysis. Without `gh` or equivalent GitHub API access, explicitly state
that conclusions about cost, pull-request frequency, pull-request size, and recurring
regressions are incomplete and less reliable.

When `gh` is available, collect only bounded metadata. Never request, print, or expose
secrets. Use the current repository and, when reviewing a pull request, its number:

```bash
gh pr view <number> --json number,title,commits,additions,deletions,changedFiles,files
gh pr list --state merged --limit 50 --json number,mergedAt,additions,deletions,changedFiles
gh api repos/{owner}/{repo}/commits?per_page=50
```

Use this data to estimate pull-request frequency, PR size, likely Cururu chunks and
LLM calls, recurring fixes, regressions, and high-risk areas. Do not claim to have
inspected history that was not provided. Distinguish observed facts from estimates
and assumptions.

## Review and model analysis

Evaluate the proposed integration against correctness, regressions, authentication,
authorization, secrets, injection, supply-chain security, latency, diff limits,
context size, retries, API rate limits, provider pricing, token usage, monthly spend,
false-positive tolerance, finding confidence, SARIF evidence, and GitHub Check Run
evidence. Consult the project's conventions, specifications, ADRs, and skills.

Recommend the least expensive model that meets the project's risk and quality needs.
Explain the trade-off between model quality, context size, chunk size, output tokens,
review frequency, and total cost. Prefer bounded settings and report low, expected,
and high monthly estimates. Flag pricing assumptions that depend on current provider
pricing.

## Output

Return a concise integration proposal containing:

1. answers still required from the owner;
2. evidence inspected and evidence unavailable;
3. recommended provider and model with alternatives;
4. estimated per-PR and monthly cost;
5. security and operational risks;
6. recommended `.cururu.toml` values and their rationale;
7. a GitHub Actions workflow using `pull_request_target`, least-privilege permissions,
   a timeout, and a pinned Cururu Action reference;
8. validation steps for the first rollout.

Never put API keys, tokens, or other secrets in `.cururu.toml`, workflow source,
prompt output, or repository comments.
