//! The tool end to end, on a plugin directory with fake platform builds.

use std::path::Path;
use std::process::Command;

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

/// `pack` for the two fake builds of [`process_plugin`].
const PACK: &[&str] = &[
    "pack",
    "--bin",
    "aarch64-apple-darwin=build/mac",
    "--bin",
    "x86_64-pc-windows-msvc=build/win.exe",
];

#[test]
fn keygen_pack_sign_verify_round_trip() {
    let dir = process_plugin();
    let public = keypair(dir.path(), "k.key");

    let (ok, out) = tool(PACK, dir.path());
    assert!(ok, "{out}");
    assert!(out.contains("proc 1.0.0 (ABI process/1.0)"), "{out}");
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
    let dir = process_plugin();
    keypair(dir.path(), "k.key");
    let other = keypair(dir.path(), "other.key");
    assert!(tool(PACK, dir.path()).0);
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
    let dir = process_plugin();
    std::fs::write(
        dir.path().join("locales/nl-BE.ftl"),
        "proc-ok = ja\nsettings-title = gestolen\n",
    )
    .unwrap();
    let (ok, out) = tool(PACK, dir.path());
    assert!(!ok && out.contains("settings-title"), "{out}");
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
                 "repo": "friendlyflow/hello-plugin-sicompass", "pubkey": "{plugin_pk}" }} ] }}"#
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
        .replace("hello-plugin", "evil-plugin");
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

/// A plugin process's directory, with two fake platform builds beside it.
fn process_plugin() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plugin.json"),
        r#"{ "name": "proc", "displayName": "proc", "type": "process", "entry": "plugin",
             "version": "1.0.0", "permissions": { "process": ["git"] } }"#,
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("locales")).unwrap();
    std::fs::write(dir.path().join("locales/en-US.ftl"), "proc-hello = hi\n").unwrap();
    std::fs::create_dir(dir.path().join("build")).unwrap();
    std::fs::write(dir.path().join("build/mac"), b"macho bytes").unwrap();
    std::fs::write(dir.path().join("build/win.exe"), b"MZ bytes").unwrap();
    dir
}

#[test]
fn a_plugin_process_packs_one_archive_per_target_and_verifies() {
    let dir = process_plugin();
    let public = keypair(dir.path(), "k.key");
    let (ok, out) = tool(
        &[
            "pack",
            "--bin",
            "aarch64-apple-darwin=build/mac",
            "--bin",
            "x86_64-pc-windows-msvc=build/win.exe",
        ],
        dir.path(),
    );
    assert!(ok, "{out}");
    assert!(out.contains("proc 1.0.0 (ABI process/1.0)"), "{out}");
    assert!(
        out.contains("plugin.exe for x86_64-pc-windows-msvc"),
        "{out}"
    );
    for f in [
        "plugin-aarch64-apple-darwin.tar.gz",
        "plugin-x86_64-pc-windows-msvc.tar.gz",
        "release.json",
    ] {
        assert!(dir.path().join("dist").join(f).is_file(), "{f}");
    }
    assert!(!dir.path().join("dist/plugin.tar.gz").exists());

    assert!(tool(&["sign", "--key", "k.key"], dir.path()).0);
    let (ok, out) = tool(&["verify", "--pubkey", &public], dir.path());
    assert!(ok, "{out}");
    assert!(
        out.contains("verifies for aarch64-apple-darwin, x86_64-pc-windows-msvc"),
        "{out}"
    );

    // Swapping one target's archive for another's breaks only that hash, and
    // verify notices.
    std::fs::copy(
        dir.path().join("dist/plugin-aarch64-apple-darwin.tar.gz"),
        dir.path().join("dist/plugin-x86_64-pc-windows-msvc.tar.gz"),
    )
    .unwrap();
    let (ok, out) = tool(&["verify", "--pubkey", &public], dir.path());
    assert!(!ok && out.contains("SHA-256"), "{out}");
}

#[test]
fn a_plugin_process_needs_known_targets_and_a_wasm_manifest_is_refused() {
    let dir = process_plugin();
    let (ok, out) = tool(&["pack"], dir.path());
    assert!(!ok && out.contains("--bin"), "{out}");
    let (ok, out) = tool(&["pack", "--bin", "x86_64-linux=build/mac"], dir.path());
    assert!(!ok && out.contains("not `x86_64-linux`"), "{out}");
    let (ok, out) = tool(&["pack", "--bin", "build/mac"], dir.path());
    assert!(!ok && out.contains("<target-triple>=<executable>"), "{out}");

    std::fs::write(
        dir.path().join("plugin.json"),
        r#"{ "name": "proc", "displayName": "proc", "entry": "plugin.wasm", "version": "1.0.0" }"#,
    )
    .unwrap();
    let (ok, out) = tool(
        &["pack", "--bin", "aarch64-apple-darwin=build/mac"],
        dir.path(),
    );
    assert!(
        !ok && out.contains("`wasm` is no longer supported"),
        "{out}"
    );
}
