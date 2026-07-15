# Release process

Cururu distributes a pre-built Docker image via GitHub Container Registry.
The `action.yml` in each version tag references the corresponding GHCR image,
so consumers never build from source.

## Prerequisites

You need the `gh` CLI and a classic PAT with `write:packages` scope, or rely on
the `GITHUB_TOKEN` which works under `packages: write` during the workflow.

## Steps

1. **Check CI** – ensure `cargo fmt --check`, `cargo clippy -- -D warnings`,
   `cargo build --release`, and `cargo test` all pass.

2. **Choose version** – follow [semver](https://semver.org/). Bump the version
   in `Cargo.toml` (e.g. `1.0.2`).

3. **Update action.yml** – change the `image:` field to match the new version:

   ```yaml
   image: docker://ghcr.io/lucaswilliameufrasio/cururu:v1.0.2
   ```

4. **Commit and push** `main` with the version bump and action.yml update.

5. **Tag the release** and push – this triggers the release workflow which
   builds the image and pushes it to GHCR:

   ```bash
   RELEASE_VERSION=v1.0.2
   git tag -a "$RELEASE_VERSION" -m "release $RELEASE_VERSION"
   git push origin "$RELEASE_VERSION"
   ```

6. **Wait for the release workflow** to finish (check Actions tab). Once it
   succeeds, the image is available at:

   ```
   ghcr.io/lucaswilliameufrasio/cururu:v1.0.2
   ghcr.io/lucaswilliameufrasio/cururu:v1   (major tag)
   ```

7. **Update major git tag** so consumers pinned to `@v1` resolve to this
   release:

   ```bash
   MAJOR=v1
   git tag -f "$MAJOR" "$RELEASE_VERSION"
   git push -f origin "$MAJOR"
   ```

8. **Make the package public** (first release only):

   ```bash
   gh api \
     --method PATCH \
     -H "Accept: application/vnd.github+json" \
     /users/lucaswilliameufrasio/packages/container/cururu \
     -f visibility=public
   ```

   Or set visibility manually at:
   `https://github.com/users/lucaswilliameufrasio/packages/container/cururu/settings`

9. **Verify** the action works in a downstream test repo by opening a PR.
   The Docker image is pulled from GHCR — no build step on the consumer side.

## Branching

- `main` is the active development branch.
- All releases are tagged from `main`.
- The `v1` major tag is a floating pointer to the latest `v1.x.y` release.
