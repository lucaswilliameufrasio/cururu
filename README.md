# Cururu

<p align="center"><img src="assets/cururu-logo.svg" alt="Cururu" width="168"></p>

![cururu-github](https://the-counter.lucaswilliameufrasio.com/v1/badges/cururu-github?label=Visualiza%C3%A7%C3%B5es&label_color=%23555&color=%2350c700)

Cururu is a self-service Rust reviewer for hosted change requests. Its core uses
pluggable source-control and LLM interfaces; the current release ships GitHub
Action, CLI, and installable GitHub App adapters. Each repository controls its
policy, model and context; secrets remain with the Action owner or App operator.

```text
pull_request event
  -> GitHub Actions
  -> cururu action (Docker)
  -> GitHub API diff
  -> LLM review (OpenAI / OpenRouter / Groq)
  -> PR summary comment with usage
```

## Quick start

### Agent review input (experimental)

`cururu prepare-review --local` emits a bounded JSON snapshot of tracked staged
and unstaged changes relative to HEAD. It excludes untracked files and lists
paths ignored by Cururu. `cururu prepare-review --pr` uses the configured GitHub
change request and checks that its head did not change during collection.
Both commands only prepare input: they do not call an agent or publish. Their
output contains source code; treat it as sensitive. Local review currently
rejects shared configuration bases rather than silently ignoring them.

`cururu mcp` offers the same read-only `prepare_review` tool to MCP hosts over
stdio, plus `validate_review` to check finding shape and anchors against a
fresh matching effective snapshot hash. `assemble_review` combines exactly one
validated result for each chunk; incomplete, reordered or stale results fail.
It strips agent-supplied analyzer provenance and follows the configured policy
for executable suggestions.
Validation does not establish correctness or approve
the change. The tools do not publish, and an MCP host may send tool output to
its configured model: only request a review when that disclosure is approved.
See [MCP client setup](docs/agent-mcp-setup.md) for non-destructive configuration
examples for OpenCode V2, Codex, Cursor, and Claude Code.
The snapshot describes its scope, ignored paths, SHA-256 and Cururu's review
instructions. Repository content is untrusted data, never an instruction.

The former experimental `agent-review` OpenCode server command is retired and
fails without contacting the server, even when `--confirm-send` is supplied.
Use `api-review` with your own configured API credentials, or ask your agent to
use Cururu's MCP tools with the agent's own authentication and permissions.

The existing API review pipeline uses `CURURU_PROVIDER` (`openrouter`, `openai`,
`groq`, `deepseek`, `zai`, or `anthropic`) and `LLM_API_KEY`; `LLM_MODEL` and `LLM_BASE_URL`
can override the selected defaults. Treat the key as a secret and configure it
through your environment or secret manager, not in repository files. An unknown
provider is rejected instead of silently sending data to a different service.
DeepSeek and Z.AI use their OpenAI-compatible APIs; provider compatibility has
not yet been verified with live accounts. To review local tracked changes,
inspect `cururu prepare-review --local`, then run `cururu api-review
--confirm-send --expected-snapshot <snapshot_sha256>`. For a hosted PR, inspect
`cururu prepare-review --pr` and use `cururu api-review --pr --confirm-send
--expected-snapshot <snapshot_sha256>` instead. Hosted reviews also send the
configured repository context, prior feedback and analyzer evidence as untrusted
data; inspect them before consenting. This sends the diff to
the configured API, returns an advisory JSON receipt, and never publishes. The
hash option is optional but rejects changes made since the preview; results are
also rejected if the snapshot changes during the review. Local untracked files,
binary files and submodules are excluded or rejected as described by the scope.
Anthropic uses its native Messages API with an Anthropic API key; this initial
adapter has not been verified against a live account. `api-review` includes
token usage when every chunk reports it; otherwise the total is omitted. For
Anthropic, input totals include cache writes and reads, and cost is unavailable.
An
agent's subscription or login is used by that agent through MCP, not by this API
pipeline.
The initial adapter sends bounded diff chunks, Cururu's canonical prompt and
configured review preferences (language, tone, audience, suggestion detail and
focus). For hosted PRs it also collects explicitly configured repository context
from the base revision and sends it as untrusted data; the snapshot includes the
context and its truncation/skipped diagnostics. Inspect `prepare-review --pr`
before confirming disclosure. Local review does not collect repository context.
When automatic context is enabled, the adapter also includes selected base files
within its configured limits; review these additional files before consenting.
Paths absent from the base tree are reported as skipped (including new or renamed
paths); a truncated tree or failure to read a file known to exist stops the review.
The adapter also includes prior review feedback as untrusted evidence, with a
bounded size and snapshot binding. Inspect `prepare-review --pr` before sending:
feedback can contain names and comments. When feedback exceeds limits on threads,
replies, individual comment length, location length, or total bytes, collection
fails instead of silently dropping or shortening it. When analysis and check runs are enabled,
matching check-run annotations are bounded, snapshot-bound and sent as untrusted
evidence. `prepare-review` accepts explicitly chosen local `--sarif FILE
--analysis-manifest FILE`; the manifest must identify the reviewed HEAD, and
paths inside it are never followed. Supply the same two options to `api-review`;
the evidence is sent only after `--confirm-send` and both files are re-read before
the result is accepted. A matching HEAD does not prove a SARIF file analyzed
uncommitted changes. To publish a receipt with local evidence, pass the same
explicit `--sarif` and `--analysis-manifest` options to `publish-agent-review`;
both files are revalidated before the preview and again after confirmation.
Selected analyzer evidence is promoted into findings subject to the configured
review policy. API assembly applies the
shared confidence, severity, maximum-findings and synthesis policies, but differs
from the hosted pipeline: both validate findings against their chunks and strip
model-supplied analyzer provenance and honor the configured suggested-changes
policy; API assembly does not run the optional evaluator. These flows are not
equivalent. Binary and submodule changes are rejected.
Configured repository context, prior feedback, and check-run evidence are
collected before an API review and included in its snapshot; explicit local
SARIF is included only when selected. The hosted pipeline also loads configured
SARIF paths and manifest paths, whereas `api-review` does not follow those paths
implicitly. OpenAI-compatible review requests currently retry failed transport,
HTTP, and response-parsing calls up to three times; a provider may have processed
an earlier attempt, so retries can incur additional usage. Anthropic's native
review adapter does not currently retry. A failed chunk returns no review receipt.
If configured analyzer evidence exceeds the finding limit, the hosted review
fails explicitly rather than silently discarding excess SARIF or annotations;
the API review also rejects excess explicitly selected evidence.
The hosted pipeline combines analyzer findings after model inference and does
not send those findings to the model. By contrast, `api-review` sends its
selected analyzer evidence as untrusted input and also includes it in the
candidate findings. These are distinct disclosure behaviors.
The hosted review rechecks the trusted base revision and hashes of both the
repository configuration and any shared base configuration before and after
model inference. A changed or unreadable shared configuration stops the review;
the shared configuration client uses the same access identity as the initial load.

### Review flow differences

| Boundary | `api-review` | MCP | Hosted pipeline |
| --- | --- | --- | --- |
| Inference and disclosure | Configured API; explicit `--confirm-send` | Client chooses its model and handles disclosure | Configured automation invokes its API |
| Analyzer evidence | Selected evidence is sent and promoted | Exposes prepared check-run evidence to the client and promotes it | Configured evidence is collected and promoted; not sent to the model |
| Executable suggestions | Governed by review policy | Governed by review policy | Governed by review policy |
| Evaluator | Not invoked | Not invoked | Optional configured evaluator; filtering is not part of the approved rollout |
| Publication | Separate interactive confirmation for PR receipts | No publication tool; separate CLI confirmation | Candidate shown in a Check Run; published only after a separate approval per candidate (GitHub or CLI) |

The API/MCP disclosure and separate-publication boundaries are intentional. The
other differences describe current behavior, not a decision that exact parity is
unnecessary. Evaluator filtering remains disabled pending approved data,
metrics, thresholds and rollout. Setting `CURURU_EVALUATOR_MODE=filter` causes the
hosted review to fail before collecting the review input or calling a model;
observe mode is unaffected. Synthetic tests do not establish model quality
or prove that an agent reviewed every file.

Local journey verification is covered by `tests/agent_review.rs` (CLI consent,
PR snapshot revalidation and interactive publication), `tests/mcp_stdio.rs`
(MCP protocol and chunk assembly), and the signed-webhook tests using a real
local SQLite queue and fake GitHub and model servers. The test-only webhook
evaluator scenario also exercises the HTTP evaluator adapter: a configuration
change during its response blocks all publication; an unchanged configuration
completes. The loopback evaluator endpoint exists only in test builds. These
checks do not verify live provider compatibility, real user experience, or the
GitHub API's non-atomic interval between the final check and publication.
The [local acceptance map](docs/local-review-acceptance.md) traces the agreed
requirements to entry points, tests, differences and verification limits.

This is an experimental protocol surface. For a PR receipt, `cururu
publish-agent-review receipt.json` fetches the current PR, checks the receipt's
target-bound snapshot and finding anchors, displays the exact comment and
target, and requires typing `PUBLISH` on an interactive terminal. It reloads
the trusted repository configuration and rechecks the snapshot after confirmation
before creating a new advisory conversation comment;
it never approves the PR or replaces earlier reviews. A receipt is editable and
its permitted single-line replacement is shown as escaped advisory text in the
preview and comment, not as an automatically applicable GitHub suggestion.
The policy is checked again; disabled or unsafe replacements are omitted. A
receipt is not cryptographic proof that an agent performed the work: read the preview.
The GitHub comment API cannot atomically condition posting on a PR revision, so
a change between the final check and the POST can still make the comment stale.
Submission is never automatically retried; on an uncertain result, check the PR
before retrying. The
MCP process uses its working directory and explicit environment, and does not
automatically load `.env`.

Add [`docs/examples/cururu-review.yml`](docs/examples/cururu-review.yml) as
`.github/workflows/cururu-review.yml` (or run `cururu init`), set `LLM_API_KEY`
as a repository secret, and create a `cururu-publish` environment
(**Settings → Environments**) with required reviewers. If the secret is not
configured, the review is skipped with a notice instead of failing the PR.

Publication requires a separate approval for every review (Cururu v5 and
later; v4 publishes directly):

1. The `review` job reviews the PR and comments **nothing**. It shows the exact
   candidate (summary and every inline comment) in the job summary and in a
   `Cururu review` check, and outputs its `candidate_digest`.
2. The `publish` job uses the `cururu-publish` environment, so it waits for a
   required reviewer. Approve it on the run page, or inspect and approve from a
   terminal with your own token: `cururu approve-hosted-review --pr <number>`.
3. Before posting, the job checks that the environment requires reviewers, that
   the run was approved, that any approval naming a candidate names this one,
   that the candidate file matches the review job's digest, and that the PR head
   and trusted configuration are unchanged. Otherwise nothing is published.

The workflow needs `checks: write` and `actions: read` in addition to the PR and
issue permissions. A new push cancels a run still waiting for approval. The
diff-limit notice is the only comment posted without approval; it contains no
code or review content. The candidate is visible to anyone who can read the
repository's checks and run summaries before approval. See
[hosted publication approval](docs/hosted-publication-approval.md).

### Auto-approval (opt-in)

Cururu can approve clearly low-risk pull requests (for example docs-only
changes) and ask for a human reviewer otherwise, based on deterministic rules
in `[auto_approval]`. It is disabled by default and never merges. Run
`cururu setup auto-approval` to preview a policy and check repository
requirements, and `cururu status` to see which features are enabled. See
[auto-approval](docs/auto-approval.md), including workflow outputs for your own
chat notifications.

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
max_output_tokens = 8000

[review]
max_diff_bytes = 180000
chunk_bytes = 45000
ignore = ["**/*.lock", "dist/**"]
language = "pt-BR"
tone = "neutral"
technical_level = "intermediate"
suggestion_detail = "detailed"
recommendations = false # Optional extra LLM guidance after recognized output truncation; may add provider cost.
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
per call; the default is `8000`. A review that ends with finish reason `length`
or an unparseable response usually needs a higher value, typically for large PRs
or many findings. Lower it (for example `4000`) to cap cost on repositories with
small PRs. You only pay for tokens the model actually generates, so a higher
limit raises the worst case, not every review.
`cururu setup limits` samples recent PRs and the token usage Cururu recorded on
their checks, and suggests higher limits when they would help. When a review
stops at the limit, the Action and the App explain which setting to change.
Transient provider failures (rate limits, overload, 5xx) are retried with
bounded backoff before the review fails.

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
- Every published review adds a **new** summary comment (earlier summaries stay
  as history) with the model's summary, this review's tokens/cost and the
  **PR total**. The total sums every Cururu review run recorded on the PR,
  including candidates that were never approved or published, and states how
  many reviews did not report tokens or cost. Each run's usage is stored in a
  hidden marker on its `Cururu review` check, so the total covers runs from the
  version that introduced it onward and at most GitHub's 250 listed PR
  commits; beyond that it is marked as a lower bound. Failed runs (for example,
  a truncated model response) and evaluator calls are not counted.

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
suggestion, with line-level highlights on the changed lines. Previously
published findings remain as history. On later reviews Cururu avoids posting a
finding again when the same file and normalized finding content were already
commented on, even if the line moved; new findings receive new comments.

**`summary`** — a single compact comment in the change-request conversation
with a findings table, tokens, and cost. It updates in place via a marker and
does not remove historical inline comments.

The action requires `pull-requests: write` and `issues: write` permissions to
post inline comments and the summary comment respectively.

### Summary

| `show_cost` | Show provider-reported cost |
| `review.recommendations` | Opt in to one additional LLM configuration recommendation after recognized output truncation; may incur extra provider cost. Defaults to `false`. |
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
| `LLM_MAX_OUTPUT_TOKENS` | no | `8000` | Override maximum response tokens |
| `CURURU_PROVIDER` | no | `openrouter` | Override provider name |
| `CURURU_IGNORE` | no | lockfiles, dist, build | Comma-separated globs to skip in diff |
| `CURURU_MAX_DIFF_BYTES` | no | `180000` | Hard cap for reviewed diff size |
| `CURURU_CHUNK_BYTES` | no | `45000` | Chunk size before each LLM call |
| `CURURU_LANGUAGE` | no | `pt-BR` | Review language (overrides TOML) |
| `CURURU_PROFILE` | no | `balanced` | Review profile |
| `CURURU_FAIL_ON` | no | `off` | Fail the action at a severity threshold |
| `CURURU_EVALUATOR_MODE` | no | disabled | Enable TypeSafe Jev in `observe` or `filter` mode |
| `CURURU_EVALUATOR_ALLOWED_REPOSITORIES` | when evaluator is enabled | — | Comma-separated local allowlist (`owner/repo` or full Git remote identity) authorizing code to be sent to TypeSafe |
| `TYPESAFE_API_KEY` | when evaluator is enabled | — | TypeSafe API key; keep in a local secret store or CI secret |
| `TYPESAFE_MODEL` | no | `jev-latest` | TypeSafe model alias or version |

### Optional Jev evaluator

The evaluator is disabled unless `CURURU_EVALUATOR_MODE` is explicitly set and
the current repository is listed in `CURURU_EVALUATOR_ALLOWED_REPOSITORIES`.
Keep that allowlist in the operator's local environment or trusted CI settings;
it is intentionally not accepted from `.cururu.toml`. When enabled, Cururu sends
finding metadata and changed-code evidence to TypeSafe's `/v1/systemone` API to
judge defect probability and severity. Do not authorize repositories whose code
must not be sent to this remote service. Each request is limited to 100 findings,
8,000 evidence characters per finding, and 64,000 evidence characters total;
findings beyond those limits remain unevaluated.

- `observe` records and displays Jev's judgments without changing which findings
  Cururu publishes.
- `filter` suppresses a finding when Jev selects `ignore` or gives a defect
  probability below `0.5`; it may also update the severity of findings it keeps.
  Suppressed findings and judgments remain in the dry-run JSON and the summary
  comment's audit section.

An evaluator request/configuration error fails the review rather than silently
falling back to unfiltered publication. Cururu uses a two-minute request timeout
with no internal retries; the webhook delivery queue may retry a failed review
up to five times. Jev's probability/confidence is a model judgment, not proof
that a finding is correct. Keep the evaluator key outside `.cururu.toml`;
repo-controlled configuration cannot enable it or provide keys.

The optional Action input `expected_head_sha` pins a run to the head SHA from
its triggering event. Cururu refuses to publish if that SHA no longer matches
the reviewed PR head. The generated workflow sets this input automatically;
other workflows should pass
`${{ github.event.pull_request.head.sha || '' }}` when available.

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
export CURURU_SCM_TOKEN=token_xxx
export CURURU_CHANGE_REQUEST_NUMBER=123
export LLM_API_KEY=sk_xxx

# If this checkout's origin remote does not identify the target repository:
export CURURU_REPOSITORY=owner/repo
cargo run -- print-diff
cargo run -- dry-run
cargo run -- review
cargo run -- print-config
```

The CLI resolves the repository from `CURURU_REPOSITORY` first, then from the
checkout's `origin` remote. `GITHUB_REPOSITORY` remains supported for existing
GitHub Actions workflows. `CURURU_CHANGE_REQUEST_NUMBER` and `CURURU_SCM_TOKEN`
are the provider-neutral names; `PR_NUMBER` and `GITHUB_TOKEN` remain compatible
aliases. When no change-request number is supplied, the GitHub adapter can
resolve it with an authenticated `gh pr view` in the current checkout. This
release ships the GitHub adapter; the core uses SCM and LLM
provider interfaces so other adapters can be added without rewriting review
logic.

### Using the CLI in a consuming repository

Open the [Cururu Releases page](https://github.com/lucaswilliameufrasio/cururu/releases)
and choose the latest stable release and archive matching your operating system
and architecture. Verify its published SHA-256 checksum before installing it.
Example for Linux x86_64 (replace `VERSION` with the selected release tag):

```bash
VERSION=vX.Y.Z
ASSET=cururu-x86_64-unknown-linux-gnu.tar.xz
BASE="https://github.com/lucaswilliameufrasio/cururu/releases/download/$VERSION"
curl --fail --location "$BASE/$ASSET" --output "$ASSET"
curl --fail --location "$BASE/$ASSET.sha256" --output "$ASSET.sha256"
sha256sum --check "$ASSET.sha256"
mkdir -p "$HOME/.local/bin"
tar -xJf "$ASSET" --strip-components=1 -C "$HOME/.local/bin"
export PATH="$HOME/.local/bin:$PATH"
cururu init
```

On Windows, select the matching archive from the same Releases page and verify
its checksum before extracting:

```powershell
$Version = "vX.Y.Z"
$Asset = "cururu-x86_64-pc-windows-msvc.zip"
$Base = "https://github.com/lucaswilliameufrasio/cururu/releases/download/$Version"
Invoke-WebRequest "$Base/$Asset" -OutFile $Asset
Invoke-WebRequest "$Base/$Asset.sha256" -OutFile "$Asset.sha256"
$Expected = ((Get-Content "$Asset.sha256" -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
$Actual = (Get-FileHash $Asset -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Actual -ne $Expected) { throw "Checksum verification failed for $Asset" }
Expand-Archive -Path $Asset -DestinationPath ".\cururu-cli" -Force
$Exe = Get-ChildItem ".\cururu-cli" -Filter "cururu.exe" -Recurse | Select-Object -First 1
if (-not $Exe) { throw "cururu.exe was not found in the verified archive" }
& $Exe.FullName init
```

Each release includes per-platform archives and SHA-256 checksums. To use a
different version or architecture, select the matching archive from that
release's assets and update `VERSION`/`ASSET` above. Alternatively, install the
selected release tag from source with Rust/Cargo installed:

```bash
cargo install --git https://github.com/lucaswilliameufrasio/cururu --tag "$VERSION" --locked
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
| `cururu print-diff` | Print the current change-request diff | SCM access token, active change-request context, and a repository identity from `CURURU_REPOSITORY` or the Git `origin` remote |
| `cururu dry-run` | Review a change request and print JSON without posting comments | SCM context plus `LLM_API_KEY` |
| `cururu review` | Review a change request and publish comments | SCM context plus `LLM_API_KEY` and write permissions |
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

With the self-hosted GitHub App, `/cururu approve <digest>` approves exactly the
candidate shown in the `Cururu review` check (the full 64-character digest is
required). Mention answers are posted directly, because a collaborator asked
for them explicitly.

Only exact commands are accepted. The commenter must have `write`, `maintain`,
or `admin` permission, and comment text cannot change the model, endpoint,
prompt, or secrets.

## Commands

```
cururu init           Add a starter .cururu.toml and GitHub Actions workflow
  cururu print-diff     Print the change-request diff
cururu dry-run        Review and print JSON, do not post comment
cururu review         Review and prepare a candidate for approval (no comment)
cururu publish-approved-review   Publish an Action candidate from an approved job
cururu approve-hosted-review     Inspect and approve a hosted candidate
cururu setup auto-approval       Preview (or --write) an opt-in auto-approval policy
cururu setup limits              Suggest diff and output-token limits from recent PRs
cururu status                    Show which Cururu features are enabled
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
