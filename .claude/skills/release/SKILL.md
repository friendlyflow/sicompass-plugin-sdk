---
name: release
description: Bump and publish sicompass-sdk (and sicompass-pdk) to crates.io
argument-hint: "[major|minor|patch]"
disable-model-invocation: true
model: sonnet
---

Publish a new `sicompass-sdk` (and, when needed, `sicompass-pdk`) to crates.io.

This file is also what `/release sicompass-plugin-sdk` follows when it is run
from a sicompass checkout, so `PROJECT_ROOT` is this repo's root.

**This publishes to the world.** A crates.io version is permanent: it can be
yanked but never replaced. Everything before step 6 is checking.

**IMPORTANT: Prefix every command with `cd <repo root> &&`**, using the absolute
path of whichever repo the command belongs to (this one, or `../sicompass` for
the app-side check). A `git push` that reports `Everything up-to-date` is the
usual symptom of running it in the wrong one.

## Why the order matters

The sicompass app pins `sicompass-sdk = "X.Y.Z"` from crates.io. So a change that
crosses the plugin ABI goes:

```
/commit-and-push sicompass-plugin-sdk   ->  land the code
/release sicompass-plugin-sdk           ->  publish the crate (this skill)
/release                                ->  bump the pin in sicompass, tag the app
```

Between the last two, sicompass `main` does not build from crates.io. Keep that
window short.

## Steps

1. **Clean state, both repos.** In this repo and in `../sicompass`,
   `git status --short` must be empty and
   `git rev-list --left-right --count origin/main...main` must print `0  0`.
   Every `[patch.crates-io]` in `../sicompass/Cargo.toml` must be commented out.

2. **Decide the version.** Compare `version` in `Cargo.toml` with
   `git tag --sort=-v:refname | head -1`. A **breaking** bump (0.x: `0.8.0` ->
   `0.9.0`) is required when any of these changed:
   - `wit/sicompass-plugin.wit`, since a changed export or import breaks every
     built guest. Also move the WIT `package` version.
   - a `Provider` trait method signature, which every built-in implements
   - the `Plugin` trait or `export_plugin!` in `sicompass-pdk`
   A patch bump covers docs, internals and formatting only.

3. **Bump `Cargo.toml`.** There is no CHANGELOG in this repo.

4. **Decide whether `sicompass-pdk` ships too.** Ship it whenever the WIT, the
   `Plugin` trait or `export_plugin!` changed. Bump both its `version` and its
   `sicompass-sdk = { version = "...", path = "..", ... }` pin, because
   `cargo publish` strips the path and keeps the version, so that pin is what a
   plugin author resolves.

5. **Checks** (inside `nix develop`, which has both guest targets):
   ```sh
   cargo test --all
   cargo test --test wit_contract
   cargo check --no-default-features --target wasm32-unknown-unknown
   cargo fmt --all -- --check
   ./scripts/verify-guest.sh
   ```
   Then prove the app builds against it without committing a patch:
   ```sh
   cd ../sicompass
   cargo test --workspace \
     --config 'patch.crates-io.sicompass-sdk.path="../sicompass-plugin-sdk"'
   ```

6. **Commit, push and tag.**
   ```sh
   git add -A && git commit -m "Release: bump version to X.Y.Z"
   git push origin HEAD:main
   git tag -a vX.Y.Z -m "Release vX.Y.Z" && git push origin vX.Y.Z
   ```
   The tag fires `release.yml`, which reruns the gates and tries to publish.
   **Expect its publish step to fail with `403 Forbidden`**: the repo's
   `CARGO_REGISTRY_TOKEN` is invalid (v0.1.6 to v0.8.0 all went out by hand).

7. **Publish `sicompass-sdk` by hand:** `cargo publish --dry-run`, then
   `cargo publish`.

8. **Publish `sicompass-pdk`**, if step 4 said so. It must come second, because
   crates.io has to hold the SDK version it pins:
   ```sh
   cd sicompass-pdk
   cargo publish --dry-run --target wasm32-unknown-unknown
   cargo publish --target wasm32-unknown-unknown
   ```
   It is its own workspace root, so `cargo test` at the repo root does not cover
   it, and it only really builds for a WASM target. Use `--no-verify` only as a
   last resort, and then build the packaged `.crate` for the WASM target against
   the published SDK by hand, which also confirms the `wit/` symlink
   materialized as a real file.

9. **Rebuild sicompass's committed WASM fixtures** if the WIT changed. Build
   `examples/hello-plugin` and `examples/net-plugin`, and copy the components to
   `../sicompass/src/sicompass/tests/fixtures/wasm/{hello,net}.wasm`. The header
   of sicompass's `tests/wasm_plugin.rs` has the exact commands. Also copy the WIT
   to `../sicompass/src/sicompass/wit/`: a test there asserts the two are
   byte-identical.

10. **Report.** Confirm the version is live (`cargo search sicompass-sdk`), then
    say the app side is ready: `/release` in sicompass bumps the pin.

## Notes

- `release.yml` publishes only `sicompass-sdk`. Even with a working token,
  `sicompass-pdk` has no automated path.
- A published version cannot be replaced. If a broken one goes out,
  `cargo yank` it and publish the next patch.
