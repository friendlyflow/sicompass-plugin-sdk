# Project Instructions

This repo is the SDK every sicompass provider builds on, and the canonical
definition of the plugin protocol. It holds four crates:

- `sicompass-sdk` (the root): FFON, the `Provider` trait, tags, timeline, input,
  and behind the default `host` feature the parts only the app process uses
  (platform directories, trash, localisation, registries). On crates.io.
  Behind `ipc`, `src/plugin_ipc/`: the protocol between the app and a plugin
  process (postcard in `u32`-prefixed frames). Behind `plugin`, `src/plugin/`:
  the `Plugin` trait, `main!`, and the plugin side of that protocol, which is
  what a plugin depends on (`default-features = false, features = ["plugin"]`).
  See `../sicompass/docs/process-plugins.md`.
- `sicompass-payments` (`sicompass-payments/`): cloud backup for plugins that
  keep their data in their own folder (sicompass's notes and board plugins, and
  any third party's). Snapshots, the backup protocol over the caller's `send`
  (the plugin's own HTTP client), the debounce, and `cloud::Cloud`, the whole
  service as a plugin runs it, over a small `Host` trait. It brings no threads,
  HTTP client or app configuration of its own. Its own workspace root, taken by
  git (not on crates.io). Certificates, tiers, checkout and redeem are **not**
  here, on purpose: they are the app's (sicompass's Store), and a plugin reaches
  them only through `sicompass_sdk::plugin::license`. The paywall is on the
  service, never on the data. Test it with `cargo test` in its folder. Its
  `tests/live_server.rs` runs by hand against `../server`.
- `examples/hello-plugin`: the smallest useful plugin, its own workspace root.
- `tools/sicompass-plugin`: the release tool (keygen, pack, sign, verify), its
  own workspace root. It uses the SDK's `package` feature and `plugin_abi`, the
  same code the Store runs, so a release that verifies here installs there.

`src/plugin_ipc/` (the messages), `src/plugin_abi.rs` (the protocol version, the
plugin targets, what the user approves, the locale-prefix rule) and
`src/plugin_manifest.rs` (`plugin.json`) are the single definition sicompass's
host, the plugins, the tool and the Store share. Change them there, not in a
copy.

Plugins were sandboxed WASM components until sicompass 0.3 (the `sicompass-pdk`
crate and `wit/sicompass-plugin.wit`, both deleted, in git history).

Work on it is usually driven from a sicompass checkout next to this one
(`../sicompass`), whose `/commit-and-push`, `/release`, `/sync` and
`/update-cargo` take `sicompass-plugin-sdk` as their first argument and then
follow the skills in this repo's `.claude/skills/`.

The plugin platform is being redesigned for sicompass 0.2.0 (WASI p2,
permissions, the Store). The design is `../sicompass/docs/plugin-platform.md`.

## Environment (Nix)

The toolchain comes from the flake dev shell in [flake.nix](flake.nix). Rust
comes from rust-overlay, like the plugin repos' shells. `flake.lock` pins it.

- **Check once per session**, then stick with the answer: `command -v cargo`.
  - Non-empty: the shell is inside `nix develop`, so run `cargo ...` directly.
  - Empty, or the wrong shell: prefix toolchain commands with `nix develop -c`.
- `nix develop -c <cmd>` prints a `warning: Git tree ... is dirty` line on
  stderr first. That warning is noise, not a failure.
- Evaluate the flake through `git+file://$PWD`, never a plain path (a plain path
  copies `target/` into the store and hangs), and always under `timeout`.
- `.cargo/config.toml` points every cargo-spawned process at a throwaway XDG
  tree under `target/`. Keep it: the `host` feature's `platform` module would
  otherwise have tests use the real sicompass directories.

## The protocol is canonical

`src/plugin_ipc/` is the contract between the app and every plugin. Its
`PROTOCOL_VERSION` (`src/plugin_abi.rs`) is `major.minor`, and the app and a
plugin talk only when the majors match:

- Adding a `Request` or `HostRequest` variant **at the end** is a minor bump: an
  older peer answers it `Unsupported`.
- Anything else (a field, a reordered variant, a changed record) is a major
  bump, because postcard encodes by position. Every plugin then needs a new
  release, and `PROCESS_ABI` changes with it.
- `tests/plugin_process.rs` runs a real plugin process; keep it passing.

## Generated files and what is not committed

- `Cargo.lock` is gitignored at every workspace root (a published library).
  `flake.lock` is committed.

## Code Style

Follow standard Rust idioms. Use `#[allow(...)]` sparingly and only when
justified. In `README.md`, do not use em dashes or semicolons. Use commas
instead, or split into separate sentences.

## Testing

- `cargo test --all --features plugin,package` at the root (the SDK, including
  `tests/plugin_process.rs`, which runs a real plugin process built from
  `tests/fixtures/stdio_plugin.rs`).
- `cargo check --no-default-features --features plugin`: the plugin half must
  not pull in the app's dependencies.
- `cargo test` in `tools/sicompass-plugin`, `sicompass-payments` and
  `examples/hello-plugin`, each its own workspace root.
- A change the app sees is also checked from `../sicompass` with
  `--config 'patch.crates-io.sicompass-sdk.path="../sicompass-plugin-sdk"'`,
  never by committing a `[patch]`.
- After implementing changes, always run the tests before finishing. When
  adding new code, write or update tests. If tests fail, fix the code.

## Test Integrity

- Never remove or weaken test assertions to make a failing test pass. Fix the
  code instead.
- If a test itself is genuinely wrong and needs changing, **ask the user
  first** before modifying it.

## Releasing

To crates.io, by hand, since the CI token is invalid. See
`.claude/skills/release/SKILL.md`.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
