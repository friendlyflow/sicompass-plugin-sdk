//! `sicompass-plugin`: pack, sign and verify sicompass plugin releases.
//!
//! The same tool a plugin's CI release job runs, and the same checks the Store
//! makes before installing, so a release that passes `verify` here installs
//! there. The format is in `sicompass_sdk::package`, the rules in
//! `sicompass_sdk::plugin_abi`.
//!
//! ```text
//! sicompass-plugin keygen --out ~/.config/sicompass/my-plugin.key
//! sicompass-plugin pack \
//!     --bin aarch64-apple-darwin=build/aarch64-apple-darwin/plugin \
//!     --bin x86_64-unknown-linux-musl=build/x86_64-unknown-linux-musl/plugin
//! # dist/plugin-<target>.tar.gz for each, and one dist/release.json
//! sicompass-plugin sign --key ~/.config/sicompass/my-plugin.key
//! sicompass-plugin verify --pubkey <base64>
//! ```
//!
//! A plugin is a program, built once per platform, each build named by its
//! target triple.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sicompass_sdk::package::{self, RELEASE_FILE, ReleaseInfo, SIGNATURE_FILE};
use sicompass_sdk::plugin_abi;
use sicompass_sdk::plugin_manifest::{PluginManifest, PluginType, parse_manifest};

#[derive(Parser)]
#[command(version, about = "Pack, sign and verify sicompass plugin releases")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a signing key. Writes the secret to --out (never to stdout) and
    /// prints the public key, which is what the store lists.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the public key of a secret key file.
    Pubkey {
        #[arg(long)]
        key: PathBuf,
    },
    /// Check a plugin directory and its builds, and write their archives and
    /// release.json.
    Pack {
        /// The plugin directory: plugin.json, assets/, locales/.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        /// `<target-triple>=<executable>`, once per platform the release
        /// supports.
        #[arg(long = "bin", value_name = "TARGET=PATH")]
        bins: Vec<String>,
    },
    /// Sign release.json with a secret key file, writing release.json.sig.
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(long, default_value = "dist")]
        dist: PathBuf,
    },
    /// Check a store's structure and sign it, writing `<store>.sig`.
    StoreSign {
        #[arg(long)]
        key: PathBuf,
        store: PathBuf,
    },
    /// Verify a store against one or more trusted public keys (base64).
    StoreVerify {
        #[arg(long = "pubkey", required = true)]
        pubkeys: Vec<String>,
        store: PathBuf,
    },
    /// Verify a packed and signed release the way the Store does.
    Verify {
        /// The public key (base64) the store lists for this plugin.
        #[arg(long)]
        pubkey: String,
        #[arg(long, default_value = "dist")]
        dist: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Keygen { out } => keygen(&out),
        Command::Pubkey { key } => read_secret(&key)
            .and_then(|s| package::public_key_of(&s))
            .map(|pk| println!("{pk}")),
        Command::Pack { dir, out, bins } => pack(&dir, &out, &bins),
        Command::Sign { key, dist } => sign(&key, &dist),
        Command::Verify { pubkey, dist } => verify(&pubkey, &dist),
        Command::StoreSign { key, store } => store_sign(&key, &store),
        Command::StoreVerify { pubkeys, store } => store_verify(&pubkeys, &store),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite a key",
            out.display()
        ));
    }
    let (secret, public) = package::generate_keypair()?;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    write_private(out, format!("{secret}\n").as_bytes())?;
    eprintln!(
        "secret key written to {} (keep it safe, back it up offline)",
        out.display()
    );
    println!("{public}");
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes)
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_secret(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn check_locales(
    manifest: &PluginManifest,
    read_file: &dyn Fn(&str) -> Result<Vec<u8>, String>,
    locale_files: &[String],
) -> Result<(), String> {
    for f in locale_files {
        let source = String::from_utf8(read_file(f)?).map_err(|_| format!("{f} is not UTF-8"))?;
        plugin_abi::check_locale_prefix(&manifest.name, &source).map_err(|id| {
            format!(
                "{f}: message `{id}` does not start with `{}-`",
                manifest.name
            )
        })?;
    }
    Ok(())
}

/// Archives an earlier pack may have left in `out`, which would no longer
/// match a new release.json.
fn clear_archives(out: &Path) {
    // `plugin.tar.gz`: a WASM release, from before sicompass 0.3.
    let _ = std::fs::remove_file(out.join("plugin.tar.gz"));
    if let Ok(entries) = std::fs::read_dir(out) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("plugin-") && name.ends_with(".tar.gz") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    // A signature from an earlier pack would no longer match either.
    let _ = std::fs::remove_file(out.join(SIGNATURE_FILE));
}

fn pack(dir: &Path, out: &Path, bins: &[String]) -> Result<(), String> {
    let manifest_json = String::from_utf8(read(&dir.join("plugin.json"))?)
        .map_err(|_| "plugin.json is not UTF-8".to_owned())?;
    let manifest = parse_manifest(&manifest_json).map_err(|e| format!("plugin.json: {e}"))?;
    if manifest.plugin_type != PluginType::Process {
        return Err("plugin.json must say `\"type\": \"process\"`".to_owned());
    }
    pack_process(dir, out, &manifest, bins)
}

/// The triples sicompass runs plugin processes on, so a typo in `--bin` is
/// caught when packing rather than when nobody can install the release.
const KNOWN_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

fn pack_process(
    dir: &Path,
    out: &Path,
    manifest: &PluginManifest,
    bins: &[String],
) -> Result<(), String> {
    if bins.is_empty() {
        return Err(
            "a plugin process needs at least one build: --bin <target-triple>=<executable>"
                .to_owned(),
        );
    }
    let files = package::collect_files(dir, manifest)?;
    let locales: Vec<String> = files
        .iter()
        .filter(|f| f.starts_with("locales/"))
        .cloned()
        .collect();
    check_locales(manifest, &|f| read(&dir.join(f)), &locales)?;

    let mut archives = BTreeMap::new();
    for bin in bins {
        let (target, path) = bin
            .split_once('=')
            .ok_or_else(|| format!("--bin {bin}: expected <target-triple>=<executable>"))?;
        if !KNOWN_TARGETS.contains(&target) {
            return Err(format!(
                "--bin {bin}: sicompass runs plugins on {}, not `{target}`",
                KNOWN_TARGETS.join(", ")
            ));
        }
        let exe = read(Path::new(path))?;
        let archive = package::build_process_archive(dir, &files, manifest, target, &exe)?;
        if archives.insert(target.to_owned(), archive).is_some() {
            return Err(format!("--bin names {target} twice"));
        }
    }
    let info = ReleaseInfo::new_process(manifest, &archives)?;
    let mut json = serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?;
    json.push(b'\n');

    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    clear_archives(out);
    for (target, archive) in &archives {
        std::fs::write(out.join(package::archive_file(target)), archive)
            .map_err(|e| e.to_string())?;
    }
    std::fs::write(out.join(RELEASE_FILE), &json).map_err(|e| e.to_string())?;

    println!("{} {} (ABI {})", info.name, info.version, info.abi);
    for f in &files {
        println!("  {f}");
    }
    for target in archives.keys() {
        println!("  {} for {target}", plugin_abi::executable_name(&manifest.entry, target));
    }
    println!(
        "wrote {} archives and {}",
        archives.len(),
        out.join(RELEASE_FILE).display()
    );
    Ok(())
}

fn sign(key: &Path, dist: &Path) -> Result<(), String> {
    let secret = read_secret(key)?;
    let release = read(&dist.join(RELEASE_FILE))?;
    let sig = package::sign(&release, &secret)?;
    std::fs::write(dist.join(SIGNATURE_FILE), format!("{sig}\n")).map_err(|e| e.to_string())?;
    println!("signed with {}", package::public_key_of(&secret)?);
    Ok(())
}

fn verify(pubkey: &str, dist: &Path) -> Result<(), String> {
    let release = read(&dist.join(RELEASE_FILE))?;
    let sig = String::from_utf8(read(&dist.join(SIGNATURE_FILE))?)
        .map_err(|_| "release.json.sig is not text".to_owned())?;
    let info = package::verify_release_info(&release, &sig, pubkey)?;
    if !info.is_process() {
        // `archive_for` says why.
        info.archive_for(None)?;
    }
    for target in info.targets.keys() {
        let archive = read(&dist.join(package::archive_file(target)))?;
        package::verify_release_for(&release, &sig, pubkey, Some(target), &archive)?;
        verify_archive(&info, &archive, target)?;
    }
    println!(
        "{} {} verifies for {}: signature, archive hashes, manifest, executables, locales",
        info.name,
        info.version,
        info.targets.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

/// What is inside one verified archive: a manifest that agrees with
/// release.json, the target's executable, and the locales.
fn verify_archive(info: &ReleaseInfo, archive: &[u8], target: &str) -> Result<(), String> {
    let files = package::read_archive(archive)?;
    let get = |name: &str| {
        files
            .iter()
            .find(|(p, _)| p == name)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| format!("{name} is missing from the archive"))
    };
    let manifest = parse_manifest(
        &String::from_utf8(get("plugin.json")?).map_err(|_| "plugin.json is not UTF-8")?,
    )
    .map_err(|e| format!("plugin.json in the archive: {e}"))?;
    info.matches_manifest(&manifest)?;
    let locales: Vec<String> = files
        .iter()
        .map(|(p, _)| p.clone())
        .filter(|p| p.starts_with("locales/") && p.ends_with(".ftl"))
        .collect();
    check_locales(&manifest, &|f| get(f), &locales)?;
    let exe = plugin_abi::executable_name(&manifest.entry, target);
    if get(&exe)?.is_empty() {
        return Err(format!("{exe} for {target} is empty"));
    }
    Ok(())
}

fn sig_path(store: &Path) -> PathBuf {
    let mut name = store.file_name().unwrap_or_default().to_os_string();
    name.push(".sig");
    store.with_file_name(name)
}

fn store_sign(key: &Path, store: &Path) -> Result<(), String> {
    let json = read(store)?;
    let parsed = sicompass_sdk::store::Store::parse(&json)?;
    let secret = read_secret(key)?;
    let sig = package::sign(&json, &secret)?;
    std::fs::write(sig_path(store), format!("{sig}\n")).map_err(|e| e.to_string())?;
    println!(
        "signed {} ({} plugins, {} tiers) with {}",
        store.display(),
        parsed.plugins.len(),
        parsed.tiers.len(),
        package::public_key_of(&secret)?
    );
    Ok(())
}

fn store_verify(pubkeys: &[String], store: &Path) -> Result<(), String> {
    let json = read(store)?;
    let sig = String::from_utf8(read(&sig_path(store))?)
        .map_err(|_| "the signature file is not text".to_owned())?;
    let keys: Vec<&str> = pubkeys.iter().map(String::as_str).collect();
    let c = sicompass_sdk::store::verify_store(&json, &sig, &keys)?;
    println!(
        "{} verifies: {} plugins, {} tiers",
        store.display(),
        c.plugins.len(),
        c.tiers.len()
    );
    Ok(())
}
