# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Chores

- Pin v4.5.0 OCI digest
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
