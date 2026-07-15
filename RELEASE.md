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

8. **Update major git tag** so consumers pinned to `@v1` resolve to this
   release:

   ```bash
   MAJOR=v1
   git tag -f "$MAJOR" "$RELEASE_VERSION"
   git push -f origin "$MAJOR"
   ```

9. **Create a GitHub Release** with release notes:

   ```bash
   gh release create "$RELEASE_VERSION" --generate-notes
   ```

10. **Verify** the action works in a downstream test repo by opening a PR.

## Branching

- `main` is the active development branch.
- All releases are tagged from `main`.
- The `v1` major tag is a floating pointer to the latest `v1.x.y` release.

## Architecture

- **linux/amd64** built on `ubuntu-24.04`
- **linux/arm64** built on `ubuntu-24.04-arm`
- Both images are combined into a single OCI index (multi-arch manifest)
- Build caching via `type=gha` with per-architecture scope
