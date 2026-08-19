# Release process

Cururu distributes a pre-built multi-platform Docker image via GitHub
Container Registry. The `action.yml` in each version tag references the image
by digest for supply-chain security.

## Prerequisites

You need `gh` CLI authenticated with at least `repo` and `write:packages`
scopes. The release workflow uses `GITHUB_TOKEN` with `packages: write`.

## Steps

1. **Check CI** – ensure `cargo fmt --check`, `cargo clippy -- -D warnings`,
   `cargo build --release`, and `cargo test` all pass.

2. **Choose version** – follow [semver](https://semver.org/). Bump the version
   in `Cargo.toml` (e.g. `1.0.5`).

3. **Update action.yml** – change the `image:` field to match the new version
   tag (the digest will be updated after the image is pushed):

   ```yaml
   image: docker://ghcr.io/lucaswilliameufrasio/cururu:v1.0.5
   ```

4. **Commit and push** `main` with the version bump and action.yml update.

5. **Tag the release** and push – this triggers the Release workflow which
   builds amd64 and arm64 images in parallel on native runners and merges
   them into a multi-arch manifest:

   ```bash
   RELEASE_VERSION=v1.0.5
   git tag -a "$RELEASE_VERSION" -m "release $RELEASE_VERSION"
   git push origin "$RELEASE_VERSION"
   ```

6. **Wait for the release workflow** to finish (~5 min). Once it succeeds:

   ```
   ghcr.io/lucaswilliameufrasio/cururu:v1.0.5   (version tag)
   ghcr.io/lucaswilliameufrasio/cururu:v1        (major tag)
   ghcr.io/lucaswilliameufrasio/cururu:latest
   ```

7. **Capture the manifest digest** from the workflow summary and update
   `action.yml` to pin by digest:

   ```yaml
   image: docker://ghcr.io/lucaswilliameufrasio/cururu@sha256:...
   ```

   Commit and push this to `main`.

8. **Update major git tag** so consumers pinned to `@v1` or `@v2` resolve to
   the commit containing the pinned digest (not the release tag itself):

   ```bash
   for tag in v1 v2; do
     git tag -f "$tag" main
     git push -f origin "$tag"
   done
   ```

   Major tags (`v1`, `v2`) point to the latest release within that major
   version. The `v2` tag tracks the 2.x line; maintain `v1` separately if
   consumers still depend on it.

9. **Create a GitHub Release** with release notes:

   ```bash
   gh release create "$RELEASE_VERSION" --generate-notes
   ```

10. **Verify** the action works in a downstream test repo by opening a PR.

## Branching

- `main` is the active development branch.
- All releases are tagged from `main`.
- The `v1` major tag is a floating pointer to the latest `v1.x.y` release.

## v4 feature release checklist

The policy, quality-gate outputs, `issue_comment` trigger, suggested changes,
incremental markers, and automatic context are behavior additions and should be
released as `v4` rather than silently changing the `v3` contract. Before tagging
`v4`:

- Pin and publish the new Docker image digest in `action.yml`.
- Verify the default `fail_on = "off"` behavior in a downstream repository.
- Verify `issue_comment` permission checks with an authorized and unauthorized user.
- Verify old version-1 `.cururu.toml` files produce the balanced profile.
- Publish migration notes for opt-in `policy`, `context.auto`, and comment commands.

## v4.1 analyzer evidence

The `v4.1` minor release adds optional SARIF ingestion. Consumers still choose
and execute their own analyzers in CI; Cururu only reads configured SARIF files.
It does not autodetect or execute repository commands. The feature is additive
and disabled unless `[analysis].enabled = true`.

## v4.2 analyzer execution manifest

The `v4.2` minor release adds an optional analysis manifest that records the
lifecycle of each analyzer separately from its diagnostics:

```json
{
  "schema_version": 1,
  "commit_sha": "<head sha>",
  "tools": [
    { "name": "cargo-clippy", "status": "failed", "exit_code": 101,
      "message": "compilation failed", "sarif_path": "artifacts/clippy.sarif" }
  ]
}
```

Supported tool statuses are `passed`, `failed`, `not_run`, `skipped`, and
`timed_out`. A stale `commit_sha` is rejected when `require_current_head` is
enabled. Cururu still only reads files; it never executes analyzer commands. The
feature is additive and disabled by default.

New action outputs: `analysis_status`, `analysis_tools_total`,
`analysis_tools_failed`, `analysis_tools_not_run`, and
`analysis_findings_count`.

## v4.2.1 security fix

The `v4.2.1` patch release rebuilds the published image with `h2 v0.4.16` to
remediate `RUSTSEC-2026-0258` (unbounded empty DATA frames, published
2026-08-17). No behavior change; only the dependency lock and the published
image digest were updated. Consumers pinned by digest should update to the new
digest; consumers using `@v4` resolve automatically.

## v4.3 finding synthesis

The `v4.3` minor release strengthens how analyzer findings and LLM findings are
deduplicated and merged when `[policy].synthesis = true`:

- Findings are grouped by (path, line, rule); rules must match exactly when
  both sides carry one.
- A finding that has a deterministic `source` (an analyzer) is kept over an
  LLM finding on the same anchor, regardless of LLM confidence. The analyzer
  is treated as ground truth and is never silently dropped for a more
  confident heuristic.
- When neither side is an analyzer, the higher-confidence finding wins, as
  before.
- Findings on the same line with different analyzer rules stay separate.

## v4.4 check-run evidence

The `v4.4` minor release lets Cururu ingest analyzer evidence reported through
GitHub **Check Runs** annotations, in addition to SARIF artifacts. Configured
via `[analysis].check_runs = true` and optionally `check_run_names`. Requires
`checks: read` on the workflow token. Annotations are normalized, filtered to
changed files, and merged with LLM findings under synthesis. Opt-in and
additive.

## Architecture

- **linux/amd64** built on `ubuntu-24.04`
- **linux/arm64** built on `ubuntu-24.04-arm`
- Both images are combined into a single OCI index (multi-arch manifest)
- Build caching via `type=gha` with per-architecture scope
