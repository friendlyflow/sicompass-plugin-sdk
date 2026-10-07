# sicompass-sync

*Cloud sync for Sicompass plugins.*

This library is part of [Sicompass](https://github.com/friendlyflow/sicompass), a
keyboard-first, accessibility-first way to use your entire computer.

It is the client half of a paid sync service, for Sicompass plugins that keep
their data in their own folder. The Sicompass notes and board plugins use it to
sync with Sicompass Cloud, and a plugin of your own can use it against your
own server.

- `cloud` is the whole service as a plugin runs it: the settings switch, the
  sync row, and a sync a few seconds after the last change, every minute and
  on demand, as background tasks. Your plugin implements a small `Host` trait
  (its clock, `license`, a thread per task, and translations) and forwards its
  saves, polls and task events.
- `merkle` is the store's Merkle tree. Every object has a hash, so two copies
  of a store can tell which objects differ without comparing their contents.
  The server computes the same hashes. It also has the three-way merge a sync
  runs when two machines both changed the store.
- `sync` is one sync: push what changed here, pull what changed elsewhere, and
  merge when both did, against the copy both sides last agreed on.
- `snapshot` reads a plugin's folder into a snapshot and writes one back,
  checking every path so a sync can never write outside the folder.
- `protocol` asks the server what it holds, downloads it, and uploads only if
  no other machine got there first, over the HTTP your plugin has.
- `debounce` syncs a while after the last change, not on every keystroke.
- `row` says which message your sync row shows, from where the user stands
  with your tier.
- `usage` reads the storage and traffic the server reports.

A merge never loses an edit. A field both machines changed goes to the one that
changed last, and an object deleted on one machine and edited on the other is
kept. And the paywall is on the service, never on the data: a plugin using this
shows and saves the user's own data whether or not they pay.

## Using it

It lives in the SDK repo. Add it to your plugin by git,
pinned to a commit (or an SDK release tag):

```toml
[dependencies]
sicompass-sync = { git = "https://github.com/friendlyflow/sicompass-plugin-sdk", rev = "<commit>" }
```

Your plugin learns where the user stands, and gets the token for your own
service, from `sicompass_sdk::plugin::license`, so it never handles a
certificate. The messages the user sees come from your plugin's own locales,
under your prefix: `cloud::MESSAGES` lists the ids. The notes plugin
(`notes-plugin-sicompass`) is a complete example.

## Building from source

From this folder, in the SDK repo's dev shell:

```bash
nix develop                            # the toolchain
cargo test
```

## Related repositories

- [sicompass](https://github.com/friendlyflow/sicompass), the application
- [notes-plugin-sicompass](https://github.com/friendlyflow/notes-plugin-sicompass)
  and
  [projectmanagement-plugin-sicompass](https://github.com/friendlyflow/projectmanagement-plugin-sicompass),
  which sync with it

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
