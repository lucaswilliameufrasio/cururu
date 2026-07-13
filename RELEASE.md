# Release process

Cururu is a Docker-based GitHub Action. Consumers reference the Docker image
via the action.yml in a git tag — no image registry push is needed.

## Steps

1. **Check CI** – ensure `cargo fmt --check`, `cargo clippy -- -D warnings`,
   `cargo build --release`, and `cargo test` all pass.

2. **Choose version** – follow [semver](https://semver.org/). Bump the version
   in `Cargo.toml` (e.g. `0.2.0`).

3. **Commit and push** `main` with the Cargo.toml bump and any other changes.

4. **Tag the release**:

   ```bash
   RELEASE_VERSION=v0.2.0
   git tag -a "$RELEASE_VERSION" -m "release $RELEASE_VERSION"
   git push origin "$RELEASE_VERSION"
   ```

5. **Update major tag** so consumers pinned to `@v1` get the latest patch/minor:

   ```bash
   MAJOR=v1
   git tag -f "$MAJOR" "$RELEASE_VERSION"
   git push -f origin "$MAJOR"
   ```

6. **Verify** the action works in a downstream test repo by opening a PR.

## Branching

- `main` is the active development branch.
- All releases are tagged from `main`.
- The `v1` major tag is a floating pointer to the latest `v1.x.y` release.
