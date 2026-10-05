//! A plugin release: `plugin.tar.gz`, `release.json` and its signature.
//!
//! Behind the `package` feature, because the `sicompass-plugin` tool and the Store
//! need it and a guest never does. The format (docs/plugin-platform.md §7):
//!
//! | File | Contents |
//! |---|---|
//! | `plugin.tar.gz` | `plugin.json`, the component, `assets/`, `locales/`, licence files |
//! | `release.json` | [`ReleaseInfo`]: name, version, ABI, permissions, service, the archive's SHA-256 |
//! | `release.json.sig` | Ed25519 over the exact bytes of `release.json`, base64 |
//!
//! A plugin process is a program, so its release has one archive per platform
//! instead of `plugin.tar.gz`: `plugin-<target>.tar.gz` ([`archive_file`]), each
//! with that platform's executable, and `release.json` names each by its hash in
//! `targets`. Still one `release.json` and one signature.
//!
//! The signature covers `release.json`, which pins the archive by hash. So the
//! Store can show version, permissions and tier from a small download and verify
//! the archive afterwards, and a release is one signature to check.
//!
//! Keys are Ed25519, stored as base64 of the 32-byte secret (a seed). A public key
//! is base64 of its 32 bytes, which is what the store lists.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_manifest::{Permissions, PluginManifest, Service};

/// File names inside a release, fixed so `releases/latest/download/<file>` works.
pub const ARCHIVE_FILE: &str = "plugin.tar.gz";
pub const RELEASE_FILE: &str = "release.json";
pub const SIGNATURE_FILE: &str = "release.json.sig";

/// Largest archive accepted, compressed.
pub const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
/// Largest total unpacked size, so a small archive cannot expand without bound.
pub const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
/// Most entries an archive may hold.
pub const MAX_ENTRIES: usize = 10_000;

/// `release.json`: what the Store reads before downloading the archive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub name: String,
    pub version: String,
    /// The plugin ABI the component was built for (`sicompass:plugin@...`).
    pub abi: String,
    #[serde(default)]
    pub min_app_version: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    /// Top-level and `permissions.allowedHosts`, merged, as the host sees them.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub service: Option<Service>,
    /// SHA-256 of `plugin.tar.gz`, lowercase hex. Empty for a plugin process,
    /// which names its archives in `targets`.
    #[serde(default)]
    pub archive_sha256: String,
    /// A plugin process's archives: target triple to the SHA-256 of
    /// `plugin-<target>.tar.gz`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, String>,
}

/// The archive of a plugin process's release for `target`.
pub fn archive_file(target: &str) -> String {
    format!("plugin-{target}.tar.gz")
}

impl ReleaseInfo {
    /// The release description of a manifest and an archive.
    pub fn new(manifest: &PluginManifest, archive: &[u8]) -> Result<Self, String> {
        let version = manifest
            .version
            .clone()
            .ok_or("plugin.json has no `version`, which a release needs")?;
        Ok(ReleaseInfo {
            name: manifest.name.clone(),
            version,
            abi: crate::plugin_abi::ABI_VERSION.to_owned(),
            min_app_version: manifest.min_app_version.clone(),
            permissions: manifest.permissions.clone(),
            allowed_hosts: manifest.allowed_hosts(),
            service: manifest.service.clone(),
            archive_sha256: sha256_hex(archive),
            targets: BTreeMap::new(),
        })
    }

    /// The release description of a plugin process: its manifest and one
    /// archive per target triple.
    pub fn new_process(
        manifest: &PluginManifest,
        archives: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, String> {
        if archives.is_empty() {
            return Err("a release needs an archive for at least one target".to_owned());
        }
        let mut info = ReleaseInfo::new(manifest, &[])?;
        info.abi = crate::plugin_abi::PROCESS_ABI.to_owned();
        info.archive_sha256 = String::new();
        info.targets = archives
            .iter()
            .map(|(t, a)| (t.clone(), sha256_hex(a)))
            .collect();
        Ok(info)
    }

    /// Whether this is a plugin process rather than a WASM component.
    pub fn is_process(&self) -> bool {
        self.abi.starts_with("process/")
    }

    /// The archive to download for `target` (see
    /// [`crate::plugin_abi::plugin_target`]) and the SHA-256 it must have.
    /// A WASM component has one archive for every platform.
    pub fn archive_for(&self, target: Option<&str>) -> Result<(String, &str), String> {
        if !self.is_process() {
            return Ok((ARCHIVE_FILE.to_owned(), &self.archive_sha256));
        }
        let target = target.ok_or("sicompass has no plugin builds for this platform")?;
        self.targets
            .get(target)
            .map(|sha| (archive_file(target), sha.as_str()))
            .ok_or_else(|| {
                format!(
                    "{} {} has no build for this computer ({target})",
                    self.name, self.version
                )
            })
    }

    /// What the user approves by installing this release, in the form
    /// `settings.json` records (`plugin_abi::approval_fingerprint`).
    pub fn approval_fingerprint(&self) -> String {
        let access = crate::plugin_abi::access_fingerprint(&self.allowed_hosts, &self.permissions);
        if self.is_process() {
            crate::plugin_abi::process_fingerprint(&access)
        } else {
            access
        }
    }

    /// Whether this release asks for access `installed` does not have: a host,
    /// folder, program or socket that is new. Asking for less is not "more".
    pub fn asks_for_more_than(&self, installed: &PluginManifest) -> bool {
        fn new_items(now: &[String], before: &[String]) -> bool {
            let norm = |s: &String| s.trim().to_ascii_lowercase();
            let before: std::collections::HashSet<String> = before.iter().map(norm).collect();
            now.iter().map(norm).any(|x| !before.contains(&x))
        }
        let (p, q) = (&self.permissions, &installed.permissions);
        // From the sandbox to a program with the user's rights is more access,
        // whatever the lists say.
        let leaves_the_sandbox = self.is_process()
            && installed.plugin_type != crate::plugin_manifest::PluginType::Process;
        leaves_the_sandbox
            || new_items(&self.allowed_hosts, &installed.allowed_hosts())
            || new_items(&p.filesystem, &q.filesystem)
            || new_items(&p.process, &q.process)
            || new_items(&p.sockets, &q.sockets)
    }

    /// Whether a manifest (the one inside the archive) says the same as this
    /// release about everything the user approves: name, version, access, service.
    pub fn matches_manifest(&self, m: &PluginManifest) -> Result<(), String> {
        let differs = |what: &str| {
            Err(format!(
                "plugin.json in the archive differs from release.json in its {what}"
            ))
        };
        if m.name != self.name {
            return differs("name");
        }
        if m.version.as_deref() != Some(self.version.as_str()) {
            return differs("version");
        }
        if m.permissions != self.permissions || m.allowed_hosts() != self.allowed_hosts {
            return differs("permissions");
        }
        if m.service != self.service {
            return differs("service");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Checking the component before it is installed
// ---------------------------------------------------------------------------

type Auditor = Box<dyn Fn(&[u8], &PluginManifest) -> Result<(), String> + Send + Sync>;

static AUDITOR: std::sync::RwLock<Option<Auditor>> = std::sync::RwLock::new(None);

/// Install the host's check of a component against its manifest: that it is a
/// component of this ABI and imports nothing its permissions do not grant.
///
/// Reading a component's imports takes a component-model parser, which the
/// app has (wasmtime) and the Store must not link a second time, so the app
/// registers the same audit it runs when it loads a plugin. Same shape as
/// [`crate::register_url_fetcher`].
pub fn register_component_auditor(
    f: impl Fn(&[u8], &PluginManifest) -> Result<(), String> + Send + Sync + 'static,
) {
    if let Ok(mut slot) = AUDITOR.write() {
        *slot = Some(Box::new(f));
    }
}

/// Audit `wasm` against `manifest` with the registered auditor. Refused when
/// none is registered: an unchecked component is never installed.
pub fn audit_component(wasm: &[u8], manifest: &PluginManifest) -> Result<(), String> {
    let slot = AUDITOR.read().map_err(|e| e.to_string())?;
    match slot.as_ref() {
        Some(audit) => audit(wasm, manifest),
        None => Err("this build cannot check a plugin's imports, so it installs none".to_owned()),
    }
}

// ---------------------------------------------------------------------------
// Keys and signatures
// ---------------------------------------------------------------------------

/// A fresh keypair: `(secret, public)`, both base64.
pub fn generate_keypair() -> Result<(String, String), String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no randomness available: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    Ok((B64.encode(seed), B64.encode(key.verifying_key().to_bytes())))
}

fn signing_key(secret_b64: &str) -> Result<SigningKey, String> {
    let bytes = B64
        .decode(secret_b64.trim())
        .map_err(|e| format!("secret key is not base64: {e}"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "secret key is not 32 bytes".to_owned())?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The public key (base64) belonging to a secret key (base64).
pub fn public_key_of(secret_b64: &str) -> Result<String, String> {
    Ok(B64.encode(signing_key(secret_b64)?.verifying_key().to_bytes()))
}

/// Sign `message` (the exact bytes of `release.json`). Returns base64.
pub fn sign(message: &[u8], secret_b64: &str) -> Result<String, String> {
    Ok(B64.encode(signing_key(secret_b64)?.sign(message).to_bytes()))
}

/// Check a base64 signature over `message` against a base64 public key.
pub fn verify(message: &[u8], signature_b64: &str, public_b64: &str) -> Result<(), String> {
    let pk: [u8; 32] = B64
        .decode(public_b64.trim())
        .map_err(|e| format!("public key is not base64: {e}"))?
        .try_into()
        .map_err(|_| "public key is not 32 bytes".to_owned())?;
    let key = VerifyingKey::from_bytes(&pk).map_err(|e| format!("bad public key: {e}"))?;
    let sig: [u8; 64] = B64
        .decode(signature_b64.trim())
        .map_err(|e| format!("signature is not base64: {e}"))?
        .try_into()
        .map_err(|_| "signature is not 64 bytes".to_owned())?;
    key.verify(message, &Signature::from_bytes(&sig))
        .map_err(|_| "the signature does not match this key and content".to_owned())
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Verify a whole release: the signature over `release.json`, then the archive
/// against the hash it names for this platform. Returns the parsed release on
/// success.
pub fn verify_release(
    release_json: &[u8],
    signature_b64: &str,
    public_b64: &str,
    archive: &[u8],
) -> Result<ReleaseInfo, String> {
    verify_release_for(
        release_json,
        signature_b64,
        public_b64,
        crate::plugin_abi::plugin_target(),
        archive,
    )
}

/// [`verify_release`] for the archive of `target`, which need not be this
/// platform's: the release tool checks every archive it signed.
pub fn verify_release_for(
    release_json: &[u8],
    signature_b64: &str,
    public_b64: &str,
    target: Option<&str>,
    archive: &[u8],
) -> Result<ReleaseInfo, String> {
    let info = verify_release_info(release_json, signature_b64, public_b64)?;
    let (file, expected) = info.archive_for(target)?;
    if archive.len() > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "the archive is larger than {MAX_ARCHIVE_BYTES} bytes"
        ));
    }
    let actual = sha256_hex(archive);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "the SHA-256 of {file} is {actual}, but release.json says {expected}"
        ));
    }
    Ok(info)
}

/// The signature over `release.json`, and its contents: what the Store reads
/// before it knows which archive to download.
pub fn verify_release_info(
    release_json: &[u8],
    signature_b64: &str,
    public_b64: &str,
) -> Result<ReleaseInfo, String> {
    verify(release_json, signature_b64, public_b64)?;
    serde_json::from_slice(release_json).map_err(|e| format!("release.json does not parse: {e}"))
}

// ---------------------------------------------------------------------------
// The archive
// ---------------------------------------------------------------------------

/// The files of a plugin directory that go into its archive, relative, sorted:
/// `plugin.json`, the manifest's `entry`, everything under `assets/`, the `.ftl`
/// files in `locales/`, and `LICENSE` / `THIRD-PARTY-LICENSES.html` if present.
/// A symlink anywhere is an error: an archive carries plain files only.
///
/// For a plugin process the entry is left out: each target's archive gets its
/// own build of it ([`build_process_archive`]).
pub fn collect_files(plugin_dir: &Path, manifest: &PluginManifest) -> Result<Vec<String>, String> {
    let mut files = vec!["plugin.json".to_owned()];
    let entry = safe_relative(&manifest.entry).ok_or_else(|| {
        format!(
            "`entry` must be a relative path inside the plugin: {}",
            manifest.entry
        )
    })?;
    if manifest.plugin_type != crate::plugin_manifest::PluginType::Process {
        files.push(entry);
    }
    for optional in ["LICENSE", "THIRD-PARTY-LICENSES.html"] {
        if plugin_dir.join(optional).is_file() {
            files.push(optional.to_owned());
        }
    }
    walk(plugin_dir, Path::new("assets"), &mut files, |_| true)?;
    walk(plugin_dir, Path::new("locales"), &mut files, |p| {
        p.extension().is_some_and(|x| x == "ftl")
    })?;
    for f in &files {
        let meta =
            std::fs::symlink_metadata(plugin_dir.join(f)).map_err(|e| format!("{f}: {e}"))?;
        if !meta.file_type().is_file() {
            return Err(format!("{f} is not a plain file"));
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn walk(
    root: &Path,
    rel: &Path,
    out: &mut Vec<String>,
    keep: impl Fn(&Path) -> bool + Copy,
) -> Result<(), String> {
    let dir = root.join(rel);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for e in entries.flatten() {
        let rel_path = rel.join(e.file_name());
        let ft = e
            .file_type()
            .map_err(|err| format!("{}: {err}", rel_path.display()))?;
        if ft.is_symlink() {
            return Err(format!(
                "{} is a symlink; an archive carries plain files only",
                rel_path.display()
            ));
        }
        if ft.is_dir() {
            walk(root, &rel_path, out, keep)?;
        } else if keep(&rel_path) {
            let s = rel_path
                .to_str()
                .ok_or_else(|| format!("{} is not UTF-8", rel_path.display()))?
                .replace('\\', "/");
            out.push(s);
        }
    }
    Ok(())
}

/// Build a reproducible `plugin.tar.gz` from `files` under `plugin_dir`: sorted
/// entries, mtime 0, owner 0, mode 0644. The same inputs give the same bytes, so
/// a release can be rebuilt and compared.
pub fn build_archive(plugin_dir: &Path, files: &[String]) -> Result<Vec<u8>, String> {
    let mut entries = Vec::new();
    for f in files {
        let data = std::fs::read(plugin_dir.join(f)).map_err(|e| format!("{f}: {e}"))?;
        entries.push((f.clone(), data, 0o644));
    }
    tar_gz(entries)
}

/// [`build_archive`] for one target of a plugin process: the shared `files`,
/// plus `executable` as [`crate::plugin_abi::executable_name`] of the
/// manifest's `entry`, mode 0755.
pub fn build_process_archive(
    plugin_dir: &Path,
    files: &[String],
    manifest: &PluginManifest,
    target: &str,
    executable: &[u8],
) -> Result<Vec<u8>, String> {
    let entry = safe_relative(&manifest.entry)
        .ok_or_else(|| format!("`entry` must be a relative path: {}", manifest.entry))?;
    let name = crate::plugin_abi::executable_name(&entry, target);
    let mut entries = Vec::new();
    for f in files {
        if *f == name {
            continue;
        }
        let data = std::fs::read(plugin_dir.join(f)).map_err(|e| format!("{f}: {e}"))?;
        entries.push((f.clone(), data, 0o644));
    }
    entries.push((name, executable.to_vec(), 0o755));
    tar_gz(entries)
}

/// A reproducible `tar.gz`: sorted entries, mtime 0, owner 0.
fn tar_gz(mut entries: Vec<(String, Vec<u8>, u32)>) -> Result<Vec<u8>, String> {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for (f, data, mode) in &entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut header, f, data.as_slice())
            .map_err(|e| format!("{f}: {e}"))?;
    }
    let gz = tar.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

/// `rel` as a clean relative path (forward slashes), or `None` if it is absolute,
/// climbs with `..`, or is empty.
fn safe_relative(rel: &str) -> Option<String> {
    let p = Path::new(rel);
    let mut parts = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Read every file of an archive into memory, refusing anything unsafe: an
/// absolute path or `..`, a link or device, more than [`MAX_ENTRIES`] entries, or
/// more than [`MAX_UNPACKED_BYTES`] in total. Returns `(path, bytes)`, sorted.
pub fn read_archive(archive: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if archive.len() > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "the archive is larger than {MAX_ARCHIVE_BYTES} bytes"
        ));
    }
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut out = Vec::new();
    let mut total: u64 = 0;
    for entry in tar.entries().map_err(|e| format!("not a tar.gz: {e}"))? {
        let mut entry = entry.map_err(|e| format!("corrupt archive: {e}"))?;
        let kind = entry.header().entry_type();
        let path = entry
            .path()
            .map_err(|e| format!("bad path in archive: {e}"))?;
        let raw = path.to_string_lossy().into_owned();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(format!(
                "{raw} is not a plain file (links and devices are refused)"
            ));
        }
        let rel = safe_relative(&raw).ok_or_else(|| format!("unsafe path in archive: {raw}"))?;
        if out.len() >= MAX_ENTRIES {
            return Err(format!("more than {MAX_ENTRIES} files in the archive"));
        }
        let size = entry.header().size().map_err(|e| e.to_string())?;
        total = total.saturating_add(size);
        if total > MAX_UNPACKED_BYTES {
            return Err(format!(
                "the archive unpacks to more than {MAX_UNPACKED_BYTES} bytes"
            ));
        }
        let mut data = Vec::with_capacity(size as usize);
        entry
            .by_ref()
            .take(size)
            .read_to_end(&mut data)
            .map_err(|e| format!("{rel}: {e}"))?;
        out.push((rel, data));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Write the files of an archive under `dest`, which must not exist yet.
///
/// Files come out mode 0644 whatever the archive says: the Store makes a plugin
/// process's executable runnable itself, and nothing else.
pub fn unpack(archive: &[u8], dest: &Path) -> Result<Vec<PathBuf>, String> {
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    let files = read_archive(archive)?;
    let mut written = Vec::new();
    for (rel, data) in files {
        let path = dest.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str) -> PluginManifest {
        crate::plugin_manifest::parse_manifest(&format!(
            r#"{{ "name": "demo", "displayName": "Demo", "entry": "plugin.wasm",
                 "version": "{version}", "permissions": {{ "allowedHosts": ["example.com"] }} }}"#
        ))
        .unwrap()
    }

    fn plugin_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("plugin.json"), "{}").unwrap();
        std::fs::write(d.path().join("plugin.wasm"), b"\0asm-ish").unwrap();
        std::fs::create_dir_all(d.path().join("assets/img")).unwrap();
        std::fs::write(d.path().join("assets/img/a.png"), b"png").unwrap();
        std::fs::create_dir(d.path().join("locales")).unwrap();
        std::fs::write(d.path().join("locales/en-US.ftl"), "demo-x = 1\n").unwrap();
        std::fs::write(d.path().join("locales/notes.txt"), "not shipped").unwrap();
        std::fs::write(d.path().join("secret.env"), "not shipped either").unwrap();
        d
    }

    #[test]
    fn a_release_round_trips_through_sign_and_verify() {
        let dir = plugin_dir();
        let m = manifest("1.2.3");
        let files = collect_files(dir.path(), &m).unwrap();
        assert_eq!(
            files,
            vec![
                "assets/img/a.png",
                "locales/en-US.ftl",
                "plugin.json",
                "plugin.wasm"
            ]
        );
        let archive = build_archive(dir.path(), &files).unwrap();
        let info = ReleaseInfo::new(&m, &archive).unwrap();
        let json = serde_json::to_vec_pretty(&info).unwrap();
        let (secret, public) = generate_keypair().unwrap();
        assert_eq!(public_key_of(&secret).unwrap(), public);
        let sig = sign(&json, &secret).unwrap();

        let got = verify_release(&json, &sig, &public, &archive).unwrap();
        assert_eq!(got, info);
        assert_eq!(got.allowed_hosts, vec!["example.com".to_owned()]);

        let contents = read_archive(&archive).unwrap();
        assert_eq!(contents.len(), 4);
        assert_eq!(
            contents[0],
            ("assets/img/a.png".to_owned(), b"png".to_vec())
        );
    }

    #[test]
    fn archives_are_reproducible() {
        let dir = plugin_dir();
        let files = collect_files(dir.path(), &manifest("1")).unwrap();
        assert_eq!(
            build_archive(dir.path(), &files).unwrap(),
            build_archive(dir.path(), &files).unwrap()
        );
    }

    #[test]
    fn tampering_is_caught() {
        let dir = plugin_dir();
        let m = manifest("1.0.0");
        let archive = build_archive(dir.path(), &collect_files(dir.path(), &m).unwrap()).unwrap();
        let json = serde_json::to_vec(&ReleaseInfo::new(&m, &archive).unwrap()).unwrap();
        let (secret, public) = generate_keypair().unwrap();
        let sig = sign(&json, &secret).unwrap();

        // Another key.
        let (_, other) = generate_keypair().unwrap();
        assert!(verify_release(&json, &sig, &other, &archive).is_err());
        // An edited release.json (more permissions, same signature).
        let edited = String::from_utf8(json.clone())
            .unwrap()
            .replace("example.com", "evil.com");
        assert!(verify_release(edited.as_bytes(), &sig, &public, &archive).is_err());
        // A swapped archive.
        let mut other_archive = archive.clone();
        other_archive.push(0);
        let e = verify_release(&json, &sig, &public, &other_archive).unwrap_err();
        assert!(e.contains("SHA-256"), "{e}");
    }

    #[test]
    fn an_archive_with_an_unsafe_path_is_refused() {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        // `append_data` refuses `..` itself, so write the name into the header raw.
        h.as_gnu_mut().unwrap().name[..9].copy_from_slice(b"../escape");
        h.set_cksum();
        tar.append(&h, &b"x"[..]).unwrap();
        let archive = tar.into_inner().unwrap().finish().unwrap();
        let e = read_archive(&archive).unwrap_err();
        assert!(e.contains("unsafe path"), "{e}");
    }

    #[test]
    fn a_symlink_is_not_packed() {
        let dir = plugin_dir();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.path().join("assets/link")).unwrap();
            let e = collect_files(dir.path(), &manifest("1")).unwrap_err();
            assert!(e.contains("symlink"), "{e}");
        }
    }

    #[test]
    fn the_archive_manifest_must_agree_with_release_json() {
        let info = ReleaseInfo::new(&manifest("1.0.0"), b"x").unwrap();
        assert!(info.matches_manifest(&manifest("1.0.0")).is_ok());
        assert!(info.matches_manifest(&manifest("1.0.1")).is_err());
        let mut more = manifest("1.0.0");
        more.permissions.storage = true;
        assert!(
            info.matches_manifest(&more)
                .unwrap_err()
                .contains("permissions")
        );
    }

    #[test]
    fn a_release_fingerprints_like_its_manifest_and_notices_new_access() {
        let before = manifest("1.0.0");
        let info = ReleaseInfo::new(&before, b"archive").unwrap();
        assert_eq!(
            info.approval_fingerprint(),
            crate::plugin_abi::approval_fingerprint(&before)
        );
        assert!(!info.asks_for_more_than(&before));

        // Fewer hosts is not more access.
        let none = crate::plugin_manifest::parse_manifest(
            r#"{ "name": "demo", "displayName": "Demo", "entry": "plugin.wasm", "version": "1.0.1" }"#,
        )
        .unwrap();
        assert!(
            !ReleaseInfo::new(&none, b"a")
                .unwrap()
                .asks_for_more_than(&before)
        );

        // A new host, folder, program or socket each is.
        for extra in [
            r#""allowedHosts": ["example.com", "other.org"]"#,
            r#""allowedHosts": ["example.com"], "filesystem": ["~/Documents"]"#,
            r#""allowedHosts": ["example.com"], "process": ["git"]"#,
            r#""allowedHosts": ["example.com"], "sockets": ["imap.example.com:993"]"#,
        ] {
            let m = crate::plugin_manifest::parse_manifest(&format!(
                r#"{{ "name": "demo", "displayName": "Demo", "entry": "plugin.wasm",
                     "version": "1.1.0", "permissions": {{ {extra} }} }}"#
            ))
            .unwrap();
            let newer = ReleaseInfo::new(&m, b"a").unwrap();
            assert!(newer.asks_for_more_than(&before), "{extra}");
            assert_ne!(newer.approval_fingerprint(), info.approval_fingerprint());
        }
    }

    fn process_manifest(version: &str) -> PluginManifest {
        crate::plugin_manifest::parse_manifest(&format!(
            r#"{{ "name": "demo", "displayName": "Demo", "type": "process", "entry": "plugin",
                 "version": "{version}", "permissions": {{ "process": ["git"] }} }}"#
        ))
        .unwrap()
    }

    fn process_release() -> (BTreeMap<String, Vec<u8>>, ReleaseInfo, Vec<u8>, String, String) {
        let dir = plugin_dir();
        let m = process_manifest("2.0.0");
        let files = collect_files(dir.path(), &m).unwrap();
        assert!(!files.contains(&"plugin".to_owned()), "each target brings its own");
        let mut archives = BTreeMap::new();
        for (target, exe) in [
            ("x86_64-unknown-linux-musl", &b"\x7fELF linux"[..]),
            ("aarch64-apple-darwin", b"macho"),
            ("x86_64-pc-windows-msvc", b"MZ windows"),
        ] {
            archives.insert(
                target.to_owned(),
                build_process_archive(dir.path(), &files, &m, target, exe).unwrap(),
            );
        }
        let info = ReleaseInfo::new_process(&m, &archives).unwrap();
        let json = serde_json::to_vec_pretty(&info).unwrap();
        let (secret, public) = generate_keypair().unwrap();
        let sig = sign(&json, &secret).unwrap();
        (archives, info, json, sig, public)
    }

    #[test]
    fn a_process_release_carries_one_archive_per_target_under_one_signature() {
        let (archives, info, json, sig, public) = process_release();
        assert!(info.is_process());
        assert_eq!(info.abi, crate::plugin_abi::PROCESS_ABI);
        assert_eq!(info.targets.len(), 3);
        for (target, archive) in &archives {
            let got =
                verify_release_for(&json, &sig, &public, Some(target), archive).unwrap();
            assert_eq!(got, info);
        }
        // Each archive has its target's executable, runnable, under its name.
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(
            &archives["x86_64-pc-windows-msvc"][..],
        ));
        let exe = tar
            .entries()
            .unwrap()
            .map(Result::unwrap)
            .find(|e| e.path().unwrap().to_str() == Some("plugin.exe"))
            .expect("plugin.exe");
        assert_eq!(exe.header().mode().unwrap(), 0o755);
        let names: Vec<String> = read_archive(&archives["aarch64-apple-darwin"])
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(names.contains(&"plugin".to_owned()) && names.contains(&"plugin.json".to_owned()));
    }

    #[test]
    fn a_process_release_fingerprints_like_its_manifest_and_leaving_wasm_is_more() {
        let (_, info, _, _, _) = process_release();
        assert_eq!(
            info.approval_fingerprint(),
            crate::plugin_abi::approval_fingerprint(&process_manifest("2.0.0"))
        );
        assert!(!info.asks_for_more_than(&process_manifest("1.0.0")));
        // The same plugin, same permissions, installed as a WASM component.
        let mut wasm = process_manifest("1.0.0");
        wasm.plugin_type = crate::plugin_manifest::PluginType::Wasm;
        assert!(info.asks_for_more_than(&wasm));
    }

    #[test]
    fn a_process_release_refuses_a_missing_target_and_another_targets_archive() {
        let (archives, info, json, sig, public) = process_release();
        let e = info.archive_for(Some("riscv64gc-unknown-linux-musl")).unwrap_err();
        assert!(e.contains("no build for this computer"), "{e}");
        assert!(info.archive_for(None).is_err());
        assert_eq!(
            info.archive_for(Some("aarch64-apple-darwin")).unwrap().0,
            "plugin-aarch64-apple-darwin.tar.gz"
        );
        let e = verify_release_for(
            &json,
            &sig,
            &public,
            Some("aarch64-apple-darwin"),
            &archives["x86_64-unknown-linux-musl"],
        )
        .unwrap_err();
        assert!(e.contains("SHA-256"), "{e}");
    }

    #[test]
    fn an_app_that_runs_only_wasm_reads_a_process_release_and_sees_its_abi() {
        // What an older app parses: no `targets` field it knows, `abi` it
        // compares, and an `archiveSha256` that is still present.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Old {
            abi: String,
            archive_sha256: String,
        }
        let (_, _, json, _, _) = process_release();
        let old: Old = serde_json::from_slice(&json).unwrap();
        assert_ne!(old.abi, crate::plugin_abi::ABI_VERSION);
        assert!(old.archive_sha256.is_empty());
        // And a WASM release still has no `targets` at all.
        let wasm = ReleaseInfo::new(&manifest("1.0.0"), b"x").unwrap();
        assert!(!serde_json::to_string(&wasm).unwrap().contains("targets"));
        assert_eq!(wasm.archive_for(None).unwrap().0, ARCHIVE_FILE);
    }

    #[test]
    fn nothing_is_audited_without_an_auditor_and_the_registered_one_decides() {
        let m = manifest("1.0.0");
        let err = audit_component(b"\0asm", &m).unwrap_err();
        assert!(err.contains("installs none"), "{err}");

        register_component_auditor(|wasm, m| {
            if wasm.starts_with(b"\0asm") && m.name == "demo" {
                Ok(())
            } else {
                Err("refused".to_owned())
            }
        });
        assert!(audit_component(b"\0asm", &m).is_ok());
        assert_eq!(audit_component(b"nope", &m).unwrap_err(), "refused");
    }
}
