# sicompass-plugin

Pack, sign and verify sicompass plugin releases, with the same checks the
sicompass Store makes before it installs one.

```bash
cargo install sicompass-plugin
```

| Command | What it does |
|---|---|
| `keygen --out <file>` | Create a signing key. The secret goes to the file, never to stdout, and the public key is printed. The public key is what the store lists. |
| `pubkey --key <file>` | Print the public key of a secret key file. |
| `pack [--dir <dir>] [--out <dir>]` | Check a built plugin directory (`plugin.json`, the component, `assets/`, `locales/`) and write `plugin.tar.gz` and `release.json` to `dist/`. |
| `sign --key <file>` | Sign `release.json`, writing `release.json.sig`. |
| `verify --pubkey <key>` | Verify a packed and signed release the way the Store does. |
| `store-sign --key <file> <store>` | Check a store's structure and sign it, writing `<store>.sig`. |
| `store-verify --pubkey <key> <store>` | Verify a store against one or more trusted public keys (base64). |

`sicompass-plugin <command> --help` lists every option.

## License

GPL-3.0-only. See the LICENSE file at the root of the repository.
