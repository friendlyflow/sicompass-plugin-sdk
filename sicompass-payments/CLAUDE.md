# Project Instructions

payments_plugin_sicompass was split out of the
[sicompass](https://github.com/friendlyflow/sicompass) workspace, and its git
history before that point is the history of `lib/lib_payments` there. Work on it is
usually driven from a sicompass checkout next to this one (`../sicompass`), whose
`/commit-and-push`, `/release`, `/sync` and `/update-cargo` take this repo's name
as their first argument and then follow the skills in this repo's
`.claude/skills/`.

It is the library crate `sicompass-payments`: cloud backup for sicompass
plugins that keep their data in their own folder (the notes and board plugins,
and any third party's). Everything in it runs inside a sandboxed plugin
(`wasm32-wasip2`): no threads, no HTTP client of its own (the caller passes
`send`, a plugin's `net::fetch`), no app configuration. Plugins depend on it by
git (`rev` between releases, `tag` at a release). It is never on crates.io.

What is **not** here, on purpose: certificates, tiers, checkout and redeeming a
token. They are the app's (sicompass's Store), and a plugin reaches them only
through its host's `license` interface (`standing`, and `token` for its own
service tier). The paywall is on the service, never on the data.

## Environment (Nix)

The toolchain comes from the flake dev shell in [flake.nix](flake.nix): Rust
from rust-overlay with the `wasm32-wasip2` target. Nothing is installed
system-wide.

- **Check once per session**, then stick with the answer: `command -v cargo`.
  - Non-empty: the shell is inside `nix develop`, so run `cargo ...` directly.
  - Empty: prefix every toolchain command with `nix develop -c`.
- `nix develop -c <cmd>` prints a `warning: Git tree ... is dirty` line on
  stderr first. That warning is noise, not a failure.
- Evaluate the flake through `git+file://$PWD`, never a plain path (a plain path
  copies `target/` into the store and hangs), and always under `timeout`.
- The version lives in `[package] version` in `Cargo.toml`.

## Code Style

Follow standard Rust idioms. Use `#[allow(...)]` sparingly and only when
justified. In `README.md`, do not use em dashes or semicolons. Use commas
instead, or split into separate sentences.

## Testing

- After implementing changes, always run the tests before finishing:
  `cargo test`, and `cargo check --target wasm32-wasip2`, since plugins link it.
- When adding new code, write or update tests.
- If tests fail, fix the code. Never leave a task with failing tests.

## Test Integrity

- Never remove or weaken test assertions to make a failing test pass. Fix the
  code instead.
- If a test itself is genuinely wrong and needs changing, **ask the user
  first** before modifying it.

## Releasing

A release is a `vX.Y.Z` tag on `main`. See `.claude/skills/release/SKILL.md`.
