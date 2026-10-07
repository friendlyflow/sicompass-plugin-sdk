---
name: release
description: Bump and publish sicompass-sdk to crates.io
argument-hint: "[major|minor|patch]"
disable-model-invocation: true
model: sonnet
---

Publish a new `sicompass-sdk` to crates.io.

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
   `git tag --sort=-v:refname | head -1`. A **breaking** bump (0.x: `0.9.x` ->
   `0.10.0`) is required when any of these changed, because every sibling that
   pins `sicompass-sdk = "0.9.0"` takes a new patch release automatically:
   - a **public item removed or changed** in the SDK (a function, a type, a
     variant, a `Provider` method signature)
   - the plugin protocol (`src/plugin_ipc`): a new `Request` / `HostRequest`
     variant at the end is only a minor bump of `PROTOCOL_VERSION`, anything
     else is a major one and needs a breaking SDK bump too
   - the `Plugin` trait in `src/plugin`
   A patch bump covers additions, docs, internals and formatting only.
   (0.9.1 removed `WIT_SOURCE`, `audit_component` and `PluginType::Wasm` and was
   still released as a patch: nothing pinning 0.9 used them. Do not repeat that.)

3. **Bump `Cargo.toml`**, and `tools/sicompass-plugin/Cargo.toml`'s
   `sicompass-sdk` pin to match. There is no CHANGELOG in this repo.

4. **The release tool** (`tools/sicompass-plugin`) is not on crates.io. The
   plugin repos' release workflows install it from this repo by git tag
   (`SDK_TAG` in their `release.yml` and `ci.yml`), so tag this repo before any
   plugin is released, and move `SDK_TAG` in the plugin repos when the tool changes.

5. **Checks:**
   ```sh
   cargo test --all --features plugin,package
   cargo check --no-default-features --features plugin
   cargo fmt --all -- --check
   (cd tools/sicompass-plugin && cargo test)
   (cd sicompass-sync && cargo test)
   (cd examples/hello-plugin && cargo test)
   ```
   Then prove the app builds against it without committing a patch:
   ```sh
   cd ../sicompass
   cargo test --workspace \
     --config 'patch.crates-io.sicompass-sdk.path="../sicompass-plugin-sdk"'
   ```
   And grep the siblings that pin the SDK (`sicompass-ui`, `loginsicompass`,
   `desicompass`) for anything the release removed.

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

8. **After publishing**, each plugin repo drops its temporary `[patch.crates-io]`
   git-rev section and resolves `sicompass-sdk = "X.Y.Z"` from crates.io, and the
   app does the same (`/release` in each). Their `Cargo.lock` lines change from
   a git source to the registry.

9. **Report.** Confirm the version is live (`cargo search sicompass-sdk`), then
    say the app side is ready: `/release` in sicompass bumps the pin.

## Notes

- `release.yml` publishes only `sicompass-sdk`.
- `sicompass-pdk` (the WASM kit, last published as 0.6.0) is retired and no
  longer in this repo. Do not publish it again.
- A published version cannot be replaced. If a broken one goes out,
  `cargo yank` it and publish the next patch.
