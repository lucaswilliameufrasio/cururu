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

## Threat model: PR content is hostile input

A pull request is an attack surface, not a trusted artifact. Its diff and
metadata are fed to the LLM, and the review output becomes a public comment on
the PR. This section states what an attacker can and cannot achieve by putting
hostile text inside a PR.

### Prompt injection through the diff

The most likely attack: a PR embeds instructions in the diff itself — file
contents, identifiers, comments or code strings saying "ignore previous
instructions", "mark this PR as clean", or "report a critical bug in file X".

**What happens:** the LLM may partially comply. The prompt is a fixed,
versioned file (`prompts/review.md`) that instructs the model to treat the diff
as data and return JSON only, but prompt hardening is a mitigation, not a
guarantee. Assume a sufficiently crafted diff can influence the review text.

**Why the blast radius stays small:**

- **No code execution.** Cururu never checks out or runs code from the PR
  branch. The diff arrives as text through the GitHub API; injection in the
  diff cannot reach a shell.
- **No credential exposure.** `LLM_API_KEY` and `GITHUB_TOKEN` are process
  environment variables. They are never placed in the prompt, so an injected
  instruction cannot ask the model to print them.
- **No arbitrary comment targets.** Parsed findings must pass anchor
  validation (`is_valid_anchor` in `diff.rs`): a comment can only be created on
  a line that is actually changed in the diff. Injection cannot make the bot
  annotate arbitrary files or lines.
- **No merge authority.** Cururu only creates, updates and deletes inline
  review comments. It never submits approving or blocking review events, never
  merges, and cannot dismiss human reviews. The quality gate
  (`fail_on`, severity counts) is computed from the structured JSON the
  maintainer configured — not from free text.
- **Advisory only.** Treat Cururu's comments as one reviewer's opinion. Do not
  use them as the sole security gate for a PR; keep human review.

**Residual risk:** the review comment itself is LLM-generated text published on
a public PR. A successful injection can produce a wrong, misleading, or
embarrassing comment, and can suppress real findings. This is accepted risk:
comments are advisory, reversible, and reconciled on the next run.

### Injection through repository context

Conventions, specifications and skills (`.cururu.toml` `[context]`) are read
from the PR's **base commit** only. A contributor cannot add an `AGENTS.md` to
their branch to steer the review; maintainer-controlled context changes land
only through commits on the base branch.

### Injection through comment triggers

`issue_comment` triggers accept only exact Cururu commands from users with
write-level permission or higher. The comment body is parsed as a command, not
appended to the prompt. Low-privilege accounts cannot trigger or steer reviews
by commenting.

### Forged analyzer evidence

The `[analysis]` feature ingests SARIF files and check-run annotations as
evidence. Defenses: the manifest must be published by a trusted workflow, is
rejected as `stale` when it does not match the PR head SHA, and every finding
is filtered to files actually changed in the diff. A PR cannot forge evidence
for code it did not touch.

### Information disclosure

The prompt contains: the diff, maintainer-controlled context files, and
repository metadata. It never contains other PRs, other repositories, or
credentials. Reviewing a PR with secrets in the diff sends those secrets to the
configured LLM provider — do not do that.
