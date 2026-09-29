# Architecture

> See also: [Glossary](glossary.md) for terms like anchor, base revision, and quality gate, and the [Security Policy](../SECURITY.md) for the trust model.

Cururu's core is designed around source-control (`ScmProvider`) and review-agent
(`ReviewAgent`) interfaces. GitHub and the OpenAI-compatible chat client are the
currently shipped adapters; a new host or model provider should implement the
relevant interface instead of adding platform-specific branches throughout the
review pipeline.

## Composition and review pipeline

```text
CLI / GitHub App webhook adapter
  -> config       generic SCM coordinates and LLM/review settings
  -> ScmProvider  source-control operations in Cururu domain terms
       -> GitHubClient adapter (current implementation)
  -> review.rs    bounded diff, context, feedback, analyzer evidence, findings
       -> ReviewAgent interface
            -> OpenAI-compatible LLM adapter (current implementation)
  -> quality.rs   severity/confidence/policy filtering
  -> output.rs    summary and finding comments
  -> ScmProvider  publish comments and summaries
```

The CLI and self-hosted GitHub App are composition roots. The App webhook
signature verification, GitHub event decoding, App JWT and installation-token
flow remain in the GitHub-specific adapter. After decoding an event, the worker
uses the same provider-neutral review and publication interfaces as the CLI.
The durable queue supports PostgreSQL or SQLite and persists delivery state,
not credentials.

## Provider boundaries

`src/scm.rs` defines the source-control contract and neutral values for
change-request diffs, revisions, repository paths, annotations, comments and
review feedback. `src/github.rs` maps those operations to GitHub REST API
requests, media types and payloads. GitHub-specific environment aliases and
webhook shapes are translated at the adapter boundary; core review code uses
`ScmConfig` and `ScmProvider` rather than GitHub owner/repository API fields.

Local repository discovery prefers `CURURU_REPOSITORY`, then the checkout's
Git `origin`; `GITHUB_REPOSITORY` remains a compatibility input for existing
Actions workflows. Generic SCM configuration uses `CURURU_SCM_PROVIDER`,
`CURURU_SCM_TOKEN`, `CURURU_SCM_API_URL`, `CURURU_SCM_SERVER_URL`, and
`CURURU_CHANGE_REQUEST_NUMBER`. The current build registers GitHub only. A
provider adapter may expose different capabilities, so optional operations
must be documented rather than assumed universal.

`src/agent.rs` exposes `ReviewAgent`, which returns Cururu's normalized review
domain model. The current OpenAI-compatible adapter in `agent.rs` obtains
responses through `provider.rs`; provider-specific request metadata is mapped
to neutral usage fields. Structured response validation and normalization stay
in Cururu so providers that only guarantee syntactically valid JSON do not
break the core contract.

## Diff limits and diagnostics

The GitHub adapter requests the documented diff media type. If the host rejects
the monolithic representation with HTTP 406, it falls back to paginated changed
file patches. If the host omits a patch for any changed file or the configured
file limit is exceeded, Cururu fails rather than silently claiming a complete
review. Review download, per-file chunk, and total diff limits are checked
before invoking the model; an over-limit review fails with a generic message.
Provider error details are bounded and scrubbed of credentials, repository
identifiers, host URLs and change-request numbers before being surfaced.

## Review history and head binding

Previously published Cururu findings are durable discussion history. A new
review never deletes or edits those comments; it compares prior findings with
new ones and publishes only findings not already present. Identity is the
repository-relative file plus normalized finding content, ignoring line shifts
and formatting/case-only variation. Existing comments are fetched with bounded
pagination so findings beyond the first page are considered. If the history
exceeds the adapter's safe page limit, Cururu fails explicitly rather than
reposting duplicates.

New inline comments are anchored to the analyzed revision using the provider's
revision/commit identifier where supported. The current head is checked before
and after publication, but multiple remote publication calls cannot be made
atomic when the host API does not provide a transactional operation. If the
head changes mid-run, already-published comments remain attached to the
analyzed revision and the error explains that a fresh review is required.
Summary comments also carry a revision marker.

## Trust boundaries

- Trusted base-revision configuration is used for `.cururu.toml` and repository
  context; shared configuration remains pinned to a full commit identifier.
- Change-request diffs, comments, analyzer messages and fetched code are
  untrusted data, never instructions. Cururu does not execute the diff.
- LLM output is untrusted until deserialized, normalized and policy-validated.
  Severities are restricted to Cururu's supported enum before display.
- Credentials are provided through protected environment variables or mounted
  secrets, never stored in TOML or queue payloads.
- Operational logs and diagnostics should avoid customer names, repository
  URLs, source paths, change-request numbers and credential values.

## Configuration layering

`src/config/` separates SCM, LLM provider, review policy, context, summary,
analysis and TOML schema. Precedence, most to least specific:

1. Environment variables and workflow inputs.
2. Local `.cururu.toml` from the trusted base revision.
3. Shared configuration pinned to a full commit identifier, when declared.
4. Profile defaults (`balanced`, `strict`, `security`, `minimal`).

Local scalars override shared scalars. Review ignore patterns, focus rules,
context paths and automatic-context inclusion/exclusion lists combine uniquely,
base first then local. Other arrays replace rather than append. `cururu init`
scaffolds local configuration and the current GitHub workflow adapter;
`cururu print-config` shows effective settings without secrets.
