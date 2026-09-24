//! The tool end to end, on the SDK's real example plugins.
//!
//! Needs the examples built: `./scripts/verify-guest.sh` at the repo root writes
//! `examples/*/plugin.wasm`, and CI runs it first.

use std::path::{Path, PathBuf};
use std::process::Command;

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// A copy of an example as a plugin directory: plugin.json, plugin.wasm, locales/.
fn plugin_copy(name: &str) -> tempfile::TempDir {
    let src = example(name);
    let wasm = src.join("plugin.wasm");
    assert!(
        wasm.is_file(),
        "{} is missing: run ./scripts/verify-guest.sh at the repo root first",
        wasm.display()
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(src.join("plugin.json"), dir.path().join("plugin.json")).unwrap();
    std::fs::copy(&wasm, dir.path().join("plugin.wasm")).unwrap();
    if src.join("locales").is_dir() {
        std::fs::create_dir(dir.path().join("locales")).unwrap();
        for e in std::fs::read_dir(src.join("locales")).unwrap().flatten() {
            std::fs::copy(e.path(), dir.path().join("locales").join(e.file_name())).unwrap();
        }
    }
    dir
}

fn tool(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sicompass-plugin"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// Run `keygen` and return the public key it prints on stdout (the notice about
/// the secret goes to stderr).
fn keypair(dir: &Path, file: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_sicompass-plugin"))
        .args(["keygen", "--out", file])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn keygen_pack_sign_verify_round_trip() {
    let dir = plugin_copy("hello-plugin");
    let public = keypair(dir.path(), "k.key");

    let (ok, out) = tool(&["pack"], dir.path());
    assert!(ok, "{out}");
    assert!(out.contains("hello 0.2.0 (ABI 0.2.0)"), "{out}");
    assert!(out.contains("locales/en-US.ftl"), "{out}");

    let (ok, out) = tool(&["sign", "--key", "k.key"], dir.path());
    assert!(ok, "{out}");
    let (ok, out) = tool(&["verify", "--pubkey", &public], dir.path());
    assert!(ok, "{out}");
    assert!(out.contains("verifies"), "{out}");
}

#[test]
fn keygen_never_overwrites_a_key() {
    let dir = tempfile::tempdir().unwrap();
    keypair(dir.path(), "k.key");
    let (ok, out) = tool(&["keygen", "--out", "k.key"], dir.path());
    assert!(!ok && out.contains("refusing to overwrite"), "{out}");
}

#[test]
fn verify_fails_on_a_wrong_key_or_an_edited_release() {
    let dir = plugin_copy("hello-plugin");
    keypair(dir.path(), "k.key");
    let other = keypair(dir.path(), "other.key");
    assert!(tool(&["pack"], dir.path()).0);
    assert!(tool(&["sign", "--key", "k.key"], dir.path()).0);

    let (ok, out) = tool(&["verify", "--pubkey", &other], dir.path());
    assert!(!ok && out.contains("signature"), "{out}");

    // Grant more access after signing: the signature no longer matches.
    let rel = dir.path().join("dist/release.json");
    let edited = std::fs::read_to_string(&rel)
        .unwrap()
        .replace("\"storage\": false", "\"storage\": true");
    std::fs::write(&rel, edited).unwrap();
    let public = String::from_utf8(
        Command::new(env!("CARGO_BIN_EXE_sicompass-plugin"))
            .args(["pubkey", "--key", "k.key"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let (ok, out) = tool(&["verify", "--pubkey", &public], dir.path());
    assert!(!ok, "an edited release.json must not verify: {out}");
}

#[test]
fn pack_refuses_a_foreign_locale_id() {
    let dir = plugin_copy("hello-plugin");
    std::fs::write(
        dir.path().join("locales/nl-BE.ftl"),
        "hello-ok = ja\nsettings-title = gestolen\n",
    )
    .unwrap();
    let (ok, out) = tool(&["pack"], dir.path());
    assert!(!ok && out.contains("settings-title"), "{out}");
}

#[test]
fn pack_refuses_a_network_plugin_without_allowed_hosts() {
    let dir = plugin_copy("net-plugin");
    std::fs::write(
        dir.path().join("plugin.json"),
        r#"{ "name": "net", "displayName": "net", "entry": "plugin.wasm", "version": "0.2.0" }"#,
    )
    .unwrap();
    let (ok, out) = tool(&["pack"], dir.path());
    assert!(!ok && out.contains("allowedHosts"), "{out}");
}

#[test]
fn a_store_is_checked_signed_and_verified() {
    let dir = tempfile::tempdir().unwrap();
    let public = keypair(dir.path(), "store.key");
    let (_, plugin_pk) = sicompass_sdk::package::generate_keypair().unwrap();
    std::fs::write(
        dir.path().join("store.json"),
        format!(
            r#"{{ "version": 1, "plugins": [ {{ "name": "hello",
                 "repo": "friendlyflow/hello_plugin_sicompass", "pubkey": "{plugin_pk}" }} ] }}"#
        ),
    )
    .unwrap();
    let (ok, out) = tool(
        &["store-sign", "--key", "store.key", "store.json"],
        dir.path(),
    );
    assert!(ok && out.contains("1 plugins"), "{out}");
    let (ok, out) = tool(
        &["store-verify", "--pubkey", &public, "store.json"],
        dir.path(),
    );
    assert!(ok, "{out}");

    // Edited after signing: refused.
    let path = dir.path().join("store.json");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("hello_plugin", "evil_plugin");
    std::fs::write(&path, edited).unwrap();
    let (ok, out) = tool(
        &["store-verify", "--pubkey", &public, "store.json"],
        dir.path(),
    );
    assert!(!ok && out.contains("signature"), "{out}");

    // A malformed store is not signed at all.
    std::fs::write(&path, r#"{ "version": 9 }"#).unwrap();
    let (ok, out) = tool(
        &["store-sign", "--key", "store.key", "store.json"],
        dir.path(),
    );
    assert!(!ok && out.contains("format"), "{out}");
}
