# Changelog

All notable changes to this project will be documented in this file.

## [6.2.1] - 2026-11-18

### Bug Fixes

- *(github)* Read PR discussion after the latest Cururu summary

### Chores

- Pin v6.2.0 OCI digest
- *(workflows)* Adopt Cururu v6.2.0

### Testing

- *(github)* A later Cururu notice does not replace the summary anchor

## [6.2.0] - 2026-11-17

### Bug Fixes

- *(release)* Attach dist-manifest.json without an artifact round trip
- *(config)* Reject any base_ref component ending in .lock

### Chores

- Pin v6.1.0 OCI digest
- *(workflows)* Adopt Cururu v6.1.0

### Features

- *(config)* Accept a branch or tag as the shared base_ref
- *(config)* Accept any Git branch name as base_ref and recommend a SHA

## [6.1.0] - 2026-11-17

### Bug Fixes

- *(release)* Stop the changelog generator from rewriting past releases
- *(review)* Index identifiers once and tighten definition detection
- *(review)* Count added lines that start with ++ in cross-chunk references
- *(review)* Close the known gaps of cross-chunk references

### Chores

- Pin v6.0.0 OCI digest
- *(workflows)* Adopt Cururu v6.0.0

### Documentation

- *(changelog)* Remove release commits and restore spacing in past entries

### Features

- *(review)* List where each chunk's definitions are used in other chunks

### Performance

- *(review)* Classify cross-reference mentions in one bounded pass

## [6.0.0] - 2026-11-16

### Breaking Changes

- [**breaking**] Make the per-review publication approval opt-in. Hosted
  reviews (Action and App) are now published as soon as they complete. To keep
  the v5 behaviour, set `[publication] require_approval = true` in the trusted
  `.cururu.toml` (or the shared base) and keep the `cururu-publish` environment
  with required reviewers.

### Bug Fixes

- Retry recording a published review's usage check
- Keep unsafe paths out of the codebase diff and bound the file count
- Show paths excluded by review.ignore in reports
- List review.ignore exclusions in the codebase issue
- Keep file names from redirecting report links
- Count characters, not bytes, against GitHub's body limit
- Explain the glob limit of a codebase command

### Chores

- Pin v5.2.0 OCI digest
- Adopt Cururu v5.2.0

### Features

- Review the committed codebase from the CLI and MCP
- Review the committed codebase and post the report as an issue
- Review the committed codebase from an issue command

### Refactor

- Extract the configured-API chunk review

### Testing

- Release the recorded checks lock before asserting

## [5.2.0] - 2026-11-15

### Bug Fixes

- Run git asynchronously with time and output bounds
- Validate context paths by component, allowing names with dots
- Keep removals with their replacement when splitting files
- Retry cut-off chunks with more output and drop unanchored findings

### Chores

- Pin v5.1.1 OCI digest
- Adopt Cururu v5.1.1

### Documentation

- Describe repository context in local reviews
- Record the first real-model quality runs
- Record the ReviewBench rerun of the cut-off pull requests
- Record the ReviewBench rerun with the fallbacks

### Features

- Load repository context for local reviews
- Ask for actionable, evidence-bound review comments

### Other

- Update Rust to 1.100.0 and compatible crates

### Testing

- Add synthetic review-quality scenarios and a real-model runner

## [5.1.1] - 2026-11-15

### Bug Fixes

- Bound the release download time in the ReviewBench image
- Split files larger than a chunk instead of aborting
- Leave binary and submodule changes out instead of aborting
- Retry inconsistent chunks and keep unconfirmed coverage
- Charge the diff budget per whole file when splitting
- List binary and submodule changes left out in the receipt
- Reserve the longest sub-hunk header when splitting hunks
- Report files cut by an over-long diff line as partially reviewed
- Keep a hunk whole when its line numbers would overflow

### Chores

- Pin v5.1.0 OCI digest
- Adopt Cururu v5.1.0

### Documentation

- Record the first offline ReviewBench run

### Features

- Add a ReviewBench agent adapter for the Cururu CLI

## [5.1.0] - 2026-11-13

### Bug Fixes

- Address review findings on path and line parsing
- Fail closed on analyzer findings and publish errors
- Never approve when an earlier approval stays active
- Revoke on unknown approval outcomes; keep file modes
- Revoke on any failure between approval and publication
- Honor the quality gate, escape model text, report success last
- Address review findings on retries and limit suggestions
- Report unreadable configuration in setup limits

### Chores

- Pin v5.0.1 OCI digest
- Adopt Cururu v5.0.1

### Documentation

- Cover diff, chunk and output-token limits in integration

### Features

- Raise the default model output limit to 8000 tokens
- Opt-in triage that approves low-risk PRs
- Guide auto-approval setup and show enabled features
- Show diff, chunk and model output limits in status
- Retry transient provider failures with bounded backoff
- Explain incomplete reviews with the setting to change
- Suggest diff and output-token limits from recent PRs

### Testing

- Cover approval, escalation, refusal and dismissal

## [5.0.1] - 2026-11-12

### Bug Fixes

- Build aarch64 macOS binaries on macos-15
- Stop unrelated PR comments from cancelling reviews
- Start comment reviews only for exact commands
- Validate comment commands in a gate job
- Create releases for existing tags without a target
- Always verify the existing release tag
- Publish only candidates that were stored
- Make the prepared candidate readable by later steps

### Chores

- Pin v5.0.0 OCI digest
- Adopt Cururu v5.0.0 and its approval flow

## [5.0.0] - 2026-11-11

### Breaking Changes

- [**breaking**] Require separate approval for each hosted publication

### Bug Fixes

- Update Cururu PR review action to v4.9.0
- Pin self-review to latest Cururu commit
- Satisfy CI clippy duration lint
- Bound evaluator calls and audit output
- Preserve SuggestedChange reexport
- Wrap quick start code to prevent scrolling
- Satisfy the pinned Rust 1.97 clippy lints

### CI / Build

- Track latest Cururu for self-review
- Update actions, Node 26.13.0 LTS, pnpm 12.18.0 and images

### Chores

- Pin v4.9.0 OCI digest
- Pin v4.9.0 OCI digest
- Bump the docker-actions group across 1 directory with 2 updates
- Install curated agent skills

### Documentation

- Keep SCM port in application
- Update base branch before creating feature branches

### Features

- Add optional Jev finding evaluator
- Add safe PR merge sequence workflow
- Gate remote reviews and cap evidence
- Share review agent contract
- Share OpenAI review adapter
- Expose canonical review prompt
- Share portable repository config parsing
- Accept bounded host-approved review context
- Carry approved review presentation preferences
- Admit approved normalized analysis evidence
- Add provider API review, MCP tools and diff-limit notice
- Post a summary per review with PR cost totals

### Other

- Update Rust to 1.99.1 and crates to latest majors
- Update astro to 7.4.8 and patch transitive advisories

### Refactor

- Extract review domain types to core crate
- Move diff domain types into core crate
- Move severity domain enums into core
- Move SCM review values into core
- Move quality gate evaluation into core
- Extract evaluator contracts into core
- Move judgment policy into core
- Extract static analysis result types
- Extract review context data types
- Extract finding consolidation helpers
- Extract review head consistency guard
- Extract review diff limit validation
- Move review candidate collection
- Extract diff chunking and anchor helpers
- Move unified diff parser
- Extract check-run finding mapping
- Extract SARIF finding mapping
- Centralize analysis report status
- Unify analysis path normalization
- Expose analysis manifest contract
- Add analysis manifest freshness check
- Map analysis manifests to report data
- Define analysis manifest schema version
- Share context path matching
- Share pure repository toml composition
- Share pure auto-context candidate rules
- Share pure review profile defaults
- Share pure SARIF parsing

### Testing

- Satisfy Rust 1.99 clippy assertions

## [4.9.0] - 2026-09-29

### Bug Fixes

- Harden provider-neutral code review
- Report provider costs accurately

### Chores

- Pin v4.8.0 OCI digest

### Documentation

- Pin v4.8.0 examples to OCI digest
- Use current release instructions
- Link showcase and current releases

### Features

- Add opt-in review recommendations
- Publish safe truncation diagnostics

### Refactor

- Preserve structured output failure reason

## [4.8.0] - 2026-09-27

### Bug Fixes

- Recover artifacts for an existing tag
- Discard pending review on submit failure

### Chores

- Pin v4.7.0 OCI digest
- Use v4.8.0 action in workflows

### Features

- Bind publishing to analyzed head SHA

## [4.7.0] - 2026-09-27

### Bug Fixes

- Deduplicate Cururu review workflow
- Version webhook path and add Tailscale access
- Isolate Tailscale and public Nginx ingress
- Explain responses without choices
- Address Cururu review findings
- Address remaining Cururu findings
- Harden release and proxy behavior
- Address remaining Cururu review findings
- Guard the inactive RSA dependency
- Bound verified artifact downloads
- Clear stale inline comments in summary mode
- Rate-limit collaborator mentions
- Avoid loading context for unrelated replies
- Restrict issue comments to pull requests
- Serialize container publishing
- Skip Dependabot private-base reviews
- Validate stable tags and use glob filters

### CI / Build

- Scope optional RSA advisory exception

### Chores

- Pin v4.6.0 OCI digest
- Bump taiki-e/install-action in the all-actions group

### Documentation

- Clarify private Tailscale validation path
- Explain draft review trigger policy
- Clarify credential sources by deployment mode

### Features

- Add self-hosted Cururu GitHub App and CLI
- Include GitHub discussion in follow-up reviews

## [4.6.0] - 2026-09-17

### Bug Fixes

- Refresh Cargo.lock to replace yanked chacha20 0.10.1
- Update rustls security patch
- Make analyzer evidence opt-in
- Make analyzer evidence opt-in

### CI / Build

- Automate release preparation and digest pinning
- Add sccache and buildx caching to speed up builds

### Chores

- Pin v4.5.0 OCI digest
- Sync Cargo.lock with v4.5.0

### Documentation

- Add Cururu integration guidance

## [4.5.0] - 2026-08-31

### CI / Build

- Skip cururu review when LLM_API_KEY is not configured

### Chores

- Pin v4.4.0 OCI digest
- Bump the all-actions group with 3 updates
- Bump the docker-actions group with 2 updates
- Release v4.5.0

### Documentation

- Add prompt injection threat model for hostile PRs
- Add architecture, glossary and ADR records

### Refactor

- Split module by domain

## [4.4.0] - 2026-08-19

### Features

- Ingest check-run annotations as evidence 

## [4.3.0] - 2026-08-19

### Chores

- Pin v4.3.0 OCI digest

### Features

- Strengthen finding synthesis 

## [4.2.1] - 2026-08-18

### Bug Fixes

- Bump h2 to 0.4.16 

### Chores

- Pin v4.2.0 OCI digest
- Release v4.2.1 
- Pin v4.2.1 OCI digest
- Sync Cargo.lock version to 4.2.1
- Pin v4.2.1 final OCI digest
- Exclude non-runtime files from image context
- Pin stable v4.2.1 OCI digest 

## [4.2.0] - 2026-08-18

### Chores

- Pin v4.1.0 OCI digest

### Features

- Add analyzer execution manifest 

## [4.1.0] - 2026-08-18

### Chores

- Pin v4.0.2 OCI digest

### Features

- Add SARIF analyzer evidence protocol

## [4.0.2] - 2026-08-18

### Chores

- Pin v4.0.1 OCI digest

### Features

- Broaden generic review checklist

## [4.0.1] - 2026-08-17

### Bug Fixes

- Fall back to bot identity when /user is forbidden

### Chores

- Pin v4 OCI digest
- Release v4.0.1

## [4.0.0] - 2026-08-17

### Chores

- Pin v3.0.3 OCI digest in action.yml

### Documentation

- Change Cururu action version to v3
- Add GitHub views badge to README

### Features

- Release Cururu v4

## [3.0.3] - 2026-08-10

### Chores

- Pin v3.0.2 OCI digest in action.yml

### Features

- Keep branded signature only on the summary review comment

## [3.0.2] - 2026-08-10

### Chores

- Pin v3.0.1 OCI digest in action.yml
- Trigger v3.0.1 inline review

### Features

- Add Cururu branded signature to summary and inline comments

## [3.0.1] - 2026-08-10

### Bug Fixes

- Instruct LLM to report new-file line numbers for inline anchors

### Chores

- Pin v3.0.0 OCI digest in action.yml
- Bump internal workflows to @v3

## [3.0.0] - 2026-08-10

### Breaking Changes

- [**breaking**] Inline review comments , with summary mode option

### Chores

- Pin v2.0.3 OCI digest in action.yml

### Documentation

- Require pull-requests write permission for comment posting

## [2.0.3] - 2026-08-08

### Chores

- Pin v2.0.2 OCI digest in action.yml

### Features

- Include GitHub error body in comment failure diagnostics

## [2.0.2] - 2026-08-08

### Bug Fixes

- Resolve base branch head for config instead of stale PR base sha

### Chores

- Pin v2.0.1 OCI digest in action.yml

## [2.0.1] - 2026-07-20

### Bug Fixes

- Absolute ENTRYPOINT in Dockerfile, add smoke test, skip Dependabot

### Chores

- Pin v2.0.0 OCI digest in action.yml
- Bump to v2.0.1

### Documentation

- Update consumer example to @v2, document major tag process

## [2.0.0] - 2026-07-17

### Breaking Changes

- [**breaking**] Change default provider to OpenRouter, update Groq to GPT-OSS 120B

### Chores

- Pin v1.2.0 OCI digest in action.yml

## [1.2.0] - 2026-07-17

### Chores

- Pin v1.1.0 OCI digest in action.yml

### Features

- Upgrade default model to gpt-5.6-luna, add cost estimates to docs

## [1.1.0] - 2026-07-15

### Chores

- Pin v1.0.5 OCI digest in action.yml

### Features

- Language config  via TOML + env + action input

## [1.0.5] - 2026-07-15

### Bug Fixes

- Reset model to provider default when switching via TOML; fix env-precedence tests

### Chores

- Pin image by digest, fix Docker compat, clean dead config, update release docs

### Testing

- 23 unit tests for config parsing 

## [1.0.3] - 2026-07-15

### Features

- Multiarch amd64+arm64, image metrics in release summary

## [1.0.2] - 2026-07-15

### Chores

- Update actions to latest versions, pin by SHA, bump to Rust 1.97.0, use debian:trixie

### Features

- Cargo-chef + distroless Dockerfile, fix rust-toolchain.toml to 1.97.0

## [1.0.1] - 2026-07-15

### Features

- Distribute pre-built Docker image via GHCR

## [1.0.0] - 2026-07-14

### Chores

- Add Makefile with useful dev commands
- Bump to 1.0.0, fix pr-agent.yml fork safety, harden CI audit

### Features

- Add initial code
- Rebrand to cururu, add retry logic with backoff, add Cargo.lock and dependabot

### Refactor

- Public Docker action with TOML config, context files, and multi-provider support
