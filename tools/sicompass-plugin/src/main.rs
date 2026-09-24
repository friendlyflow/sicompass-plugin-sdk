//! `sicompass-plugin`: pack, sign and verify sicompass plugin releases.
//!
//! The same tool a plugin's CI release job runs, and the same checks the Store
//! makes before installing, so a release that passes `verify` here installs
//! there. The format is in `sicompass_sdk::package`, the rules in
//! `sicompass_sdk::plugin_abi`.
//!
//! ```text
//! sicompass-plugin keygen --out ~/.config/sicompass/my-plugin.key
//! cargo build --release --target wasm32-wasip2
//! cp target/wasm32-wasip2/release/my_plugin.wasm plugin.wasm
//! sicompass-plugin pack                 # dist/plugin.tar.gz + dist/release.json
//! sicompass-plugin sign --key ~/.config/sicompass/my-plugin.key
//! sicompass-plugin verify --pubkey <base64>
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sicompass_sdk::package::{self, ARCHIVE_FILE, RELEASE_FILE, ReleaseInfo, SIGNATURE_FILE};
use sicompass_sdk::plugin_abi::{self, ImportedInterface};
use sicompass_sdk::plugin_manifest::{PluginManifest, parse_manifest};

#[derive(Parser)]
#[command(version, about = "Pack, sign and verify sicompass plugin releases")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a signing key. Writes the secret to --out (never to stdout) and
    /// prints the public key, which is what the catalog lists.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the public key of a secret key file.
    Pubkey {
        #[arg(long)]
        key: PathBuf,
    },
    /// Check a built plugin directory and write plugin.tar.gz and release.json.
    Pack {
        /// The plugin directory: plugin.json, the component, assets/, locales/.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
    },
    /// Sign release.json with a secret key file, writing release.json.sig.
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(long, default_value = "dist")]
        dist: PathBuf,
    },
    /// Verify a packed and signed release the way the Store does.
    Verify {
        /// The public key (base64) the catalog lists for this plugin.
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
        Command::Pack { dir, out } => pack(&dir, &out),
        Command::Sign { key, dist } => sign(&key, &dist),
        Command::Verify { pubkey, dist } => verify(&pubkey, &dist),
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

/// The WIT-level imports and exports of a component.
fn component_interfaces(wasm: &[u8]) -> Result<(Vec<ImportedInterface>, Vec<String>), String> {
    let decoded = wit_component::decode(wasm).map_err(|e| format!("not a WASM component: {e}"))?;
    let (resolve, world) = match &decoded {
        wit_component::DecodedWasm::Component(r, w) => (r, *w),
        wit_component::DecodedWasm::WitPackage(..) => {
            return Err("this is a WIT package, not a component".to_owned());
        }
    };
    let name_of = |item: &wit_parser::WorldItem| match item {
        wit_parser::WorldItem::Interface { id, .. } => resolve.id_of(*id).map(|n| (n, *id)),
        _ => None,
    };
    let mut imports = Vec::new();
    for item in resolve.worlds[world].imports.values() {
        if let Some((name, id)) = name_of(item) {
            imports.push(ImportedInterface {
                name,
                functions: resolve.interfaces[id].functions.keys().cloned().collect(),
            });
        }
    }
    let exports = resolve.worlds[world]
        .exports
        .values()
        .filter_map(|i| name_of(i).map(|(n, _)| n))
        .collect();
    Ok((imports, exports))
}

/// Every check the Store makes on a plugin's files, given its manifest and a
/// way to read one of its files.
fn check_plugin(
    manifest: &PluginManifest,
    read_file: &dyn Fn(&str) -> Result<Vec<u8>, String>,
    locale_files: &[String],
) -> Result<Vec<ImportedInterface>, String> {
    let wasm = read_file(&manifest.entry)?;
    let (imports, exports) = component_interfaces(&wasm)?;
    plugin_abi::audit_imports(
        &imports,
        &exports,
        &manifest.permissions,
        &manifest.allowed_hosts(),
    )?;
    for f in locale_files {
        let source = String::from_utf8(read_file(f)?).map_err(|_| format!("{f} is not UTF-8"))?;
        plugin_abi::check_locale_prefix(&manifest.name, &source).map_err(|id| {
            format!(
                "{f}: message `{id}` does not start with `{}-`",
                manifest.name
            )
        })?;
    }
    Ok(imports)
}

fn pack(dir: &Path, out: &Path) -> Result<(), String> {
    let manifest_json = String::from_utf8(read(&dir.join("plugin.json"))?)
        .map_err(|_| "plugin.json is not UTF-8".to_owned())?;
    let manifest = parse_manifest(&manifest_json).map_err(|e| format!("plugin.json: {e}"))?;
    let files = package::collect_files(dir, &manifest)?;
    let locales: Vec<String> = files
        .iter()
        .filter(|f| f.starts_with("locales/"))
        .cloned()
        .collect();
    let imports = check_plugin(&manifest, &|f| read(&dir.join(f)), &locales)?;

    let archive = package::build_archive(dir, &files)?;
    let info = ReleaseInfo::new(&manifest, &archive)?;
    let mut json = serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?;
    json.push(b'\n');

    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::write(out.join(ARCHIVE_FILE), &archive).map_err(|e| e.to_string())?;
    std::fs::write(out.join(RELEASE_FILE), &json).map_err(|e| e.to_string())?;
    // A signature from an earlier pack would no longer match.
    let _ = std::fs::remove_file(out.join(SIGNATURE_FILE));

    println!("{} {} (ABI {})", info.name, info.version, info.abi);
    for f in &files {
        println!("  {f}");
    }
    println!("imports:");
    for i in &imports {
        println!("  {}", i.name);
    }
    println!(
        "wrote {} and {}",
        out.join(ARCHIVE_FILE).display(),
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
    let archive = read(&dist.join(ARCHIVE_FILE))?;
    let info = package::verify_release(&release, &sig, pubkey, &archive)?;

    let files = package::read_archive(&archive)?;
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
    check_plugin(&manifest, &|f| get(f), &locales)?;

    println!(
        "{} {} verifies: signature, archive hash, manifest, imports, locales",
        info.name, info.version
    );
    Ok(())
}
