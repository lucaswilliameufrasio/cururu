# Changelog

All notable changes to this project will be documented in this file.

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
- Prepare for v4.9.0

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
- Prepare for v4.8.0
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
- Prepare for v4.7.0

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
- Prepare for v4.6.0

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
