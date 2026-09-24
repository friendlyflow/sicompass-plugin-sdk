# sicompass-payments

*Cloud backup for Sicompass plugins.*

This library is part of [Sicompass](https://github.com/friendlyflow/sicompass), a
keyboard-first, accessibility-first way to use your entire computer.

It is the client half of a paid backup service, for Sicompass plugins that keep
their data in their own folder. The Sicompass notes and board plugins use it to
back up to Sicompass Cloud, and a plugin of your own can use it against your
own server.

- `cloud` is the whole service as a plugin runs it: the settings switch, the
  backup row, uploads a few seconds after the last change and restores, both
  as background tasks. Your plugin implements a small `Host` trait (its clock,
  `license`, `tasks` and translations) and forwards its saves, polls and task
  events.
- `snapshot` reads a plugin's folder into a snapshot and writes one back,
  checking every path so a restore can never write outside the folder.
- `protocol` uploads, downloads and restores, over the HTTP your plugin has
  (its `net::fetch`).
- `debounce` uploads a while after the last change, not on every keystroke.
- `row` says which message your backup row shows, from where the user stands
  with your tier.
- `usage` reads the storage and traffic the server reports.

A restore never runs over a folder that already has files in it. A backup is
not a sync, and the machine in front of the user wins. And the paywall is on
the service, never on the data: a plugin using this shows and saves the user's
own data whether or not they pay.

## Using it

It lives in the SDK repo, next to `sicompass-pdk`. Add it to your plugin by git,
pinned to a commit (or an SDK release tag):

```toml
[dependencies]
sicompass-payments = { git = "https://github.com/friendlyflow/sicompass-plugin-sdk", rev = "<commit>" }
```

Your plugin learns where the user stands, and gets the token for your own
service, from its host's `license` interface, so it never handles a
certificate. The messages the user sees come from your plugin's own locales,
under your prefix: `cloud::MESSAGES` lists the ids. The notes plugin
(`notes_plugin_sicompass`) is a complete example.

## Building from source

From this folder, in the SDK repo's dev shell:

```bash
nix develop                            # the toolchain, with wasm32-wasip2
cargo test
cargo check --target wasm32-wasip2     # the build plugins link
```

## Related repositories

- [sicompass](https://github.com/friendlyflow/sicompass), the application
- [notes_plugin_sicompass](https://github.com/friendlyflow/notes_plugin_sicompass)
  and
  [projectmanagement_plugin_sicompass](https://github.com/friendlyflow/projectmanagement_plugin_sicompass),
  which back up with it

## Community

Join the conversation on
[Discord](https://discord.com/channels/1464152138753249313/1464152139231137894).

## License

#### Open source license

If you are creating an open source application under a license compatible with
the GNU GPL license v3, you may use this project under the terms of the GPLv3.
See [LICENSE](../LICENSE).

## Contributing

Contributions are welcome. Whether it is code, documentation, or feedback, your
input helps make computing more accessible for everyone.
