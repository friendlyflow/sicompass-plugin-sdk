---
name: update-cargo
description: Refresh this repo's flake.lock and check the SDK, the pdk and the examples against freshly resolved dependencies
argument-hint: "[push]"
disable-model-invocation: true
model: sonnet
effort: medium
---

Refresh and verify this repo's dependencies.

This file is also what `/update-cargo sicompass-plugin-sdk` follows when it is
run from a sicompass checkout. **Prefix every command with `cd PROJECT_ROOT &&`.**

**This repo gitignores `Cargo.lock`**, the normal convention for a published
library: CI and every consumer resolve the newest semver-compatible versions on
a fresh checkout. So there is no lockfile to commit here. The only committed lock
is `flake.lock` (the toolchain and wasm-tools).

1. `git status --short` must be empty.
2. `cargo update` at the root, in `sicompass-pdk/` and in each `examples/*`. Each
   is its own workspace root with its own untracked lockfile.
3. `nix flake update`. That moves nixpkgs and rust-overlay, so the Rust
   toolchain may move too.
4. Inside `nix develop`:
   - `cargo test --all`, and `cargo build --no-default-features` (the guest
     surface of the SDK)
   - `cargo build --target wasm32-unknown-unknown` in `sicompass-pdk/`. Do not
     run `cargo test` there, since it has no host target to run on.
   - `./scripts/verify-guest.sh`
   A failure from a bumped crate is a report, not a refactor. Name the crate and
   the configuration, and stop.
5. `git diff --stat` shows only `flake.lock`. Commit it:
   `chore: update flake.lock`, no co-author trailer.
6. With `push` in `$ARGUMENTS`: `git push origin HEAD:main`.
7. Report the toolchain version before and after, and which checks ran.
