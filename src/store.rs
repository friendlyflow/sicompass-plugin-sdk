//! The store: which plugins it offers, whose keys sign them, and the paid tiers.
//! Behind the `package` feature, next to the release format.
//!
//! It lives in the sicompass repo (`lib/lib_store/store.json` and
//! `store.json.sig`). It holds **no versions**: the Store reads each plugin's
//! signed `release.json` from its repo's latest GitHub release, so releasing a
//! plugin never needs a sicompass commit. The one exception is a frozen entry
//! (a third party whose tier lapsed), which names the release tag it stays at.
//! `store.json` changes only when a plugin, a key or a tier is added, or an
//! entry is frozen, and only a trusted store key can make the app believe it
//! (docs/plugin-platform.md §8).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const STORE_FILE: &str = "store.json";
pub const STORE_SIGNATURE_FILE: &str = "store.json.sig";

/// The store format version this SDK reads.
pub const STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub version: u32,
    /// Paid tiers by id (`friendlyflow/cloud`), each with its certificate issuer.
    #[serde(default)]
    pub tiers: BTreeMap<String, Tier>,
    #[serde(default)]
    pub plugins: Vec<StoreEntry>,
}

/// A paid tier: who issues its certificates and where to buy it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tier {
    /// Ed25519 public key (base64) that signs this tier's certificates.
    pub issuer: String,
    /// Checkout page.
    pub checkout: String,
    /// A Fluent id in the Store's bundles, e.g. `store-tier-cloud`.
    pub title: String,
}

/// One plugin the Store offers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreEntry {
    /// The plugin's manifest `name`, also its install directory.
    pub name: String,
    /// `owner/repo` on GitHub, where its releases are published.
    pub repo: String,
    /// Ed25519 public key (base64) its releases are signed with.
    pub pubkey: String,
    /// A grouping for the Store's list, e.g. `productivity`.
    #[serde(default)]
    pub category: Option<String>,
    /// A paid service the plugin uses (a tier id), shown before install.
    #[serde(default)]
    pub service: Option<String>,
    /// The plugin gates some of its own features (third parties only, disclosed).
    #[serde(default)]
    pub paid_features: bool,
    /// SHA-256 (hex) of archives that must never be installed: a leaked key's
    /// releases, or a broken one. The Store refuses them and offers the latest.
    #[serde(default)]
    pub revoked: Vec<String>,
    /// The release tag (`v1.4.0`) a third party's entry is frozen at, because
    /// the tier its listing needs lapsed: the Store installs that release and
    /// offers nothing newer. Older clients ignore the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen: Option<String>,
}

impl StoreEntry {
    /// The folder under `releases_url` (`https://github.com`) that this
    /// plugin's release files are downloaded from, ending in `/`: the latest
    /// release, or the frozen one.
    pub fn release_folder(&self, releases_url: &str) -> String {
        match &self.frozen {
            Some(tag) => format!("{releases_url}/{}/releases/download/{tag}/", self.repo),
            None => format!("{releases_url}/{}/releases/latest/download/", self.repo),
        }
    }

    /// Where a file of this plugin's release is downloaded from: the latest
    /// one, or the one the entry is frozen at.
    pub fn release_url(&self, file: &str) -> String {
        format!("{}{file}", self.release_folder("https://github.com"))
    }

    /// Whether an archive with this SHA-256 has been revoked.
    pub fn is_revoked(&self, archive_sha256: &str) -> bool {
        self.revoked
            .iter()
            .any(|r| r.eq_ignore_ascii_case(archive_sha256))
    }
}

impl Store {
    /// Parse and check the structure: the format version, unique names, sane
    /// repos and keys, and every service naming a listed tier.
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let c: Store =
            serde_json::from_slice(json).map_err(|e| format!("store does not parse: {e}"))?;
        if c.version != STORE_VERSION {
            return Err(format!(
                "store format {} is not the one this sicompass reads ({STORE_VERSION})",
                c.version
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for p in &c.plugins {
            if !seen.insert(p.name.as_str()) {
                return Err(format!("store lists `{}` twice", p.name));
            }
            check_entry_fields(&p.name, &p.repo, &p.pubkey)?;
            if let Some(tag) = &p.frozen {
                check_tag(tag).map_err(|e| format!("`{}`: {e}", p.name))?;
            }
            if let Some(tier) = &p.service
                && !c.tiers.contains_key(tier)
            {
                return Err(format!(
                    "`{}`: service tier `{tier}` is not in the store",
                    p.name
                ));
            }
        }
        for (id, t) in &c.tiers {
            check_key(&t.issuer).map_err(|e| format!("tier `{id}`: {e}"))?;
        }
        Ok(c)
    }

    pub fn entry(&self, name: &str) -> Option<&StoreEntry> {
        self.plugins.iter().find(|p| p.name == name)
    }
}

/// Check what a store entry is made of: a plain name, an `owner/name` repo and
/// a 32-byte base64 key. The Store's publish form checks a submission with it
/// before sending it, so the developer hears the same words `store-sign` uses.
pub fn check_entry_fields(name: &str, repo: &str, pubkey: &str) -> Result<(), String> {
    let valid_name = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if !valid_name {
        return Err(format!("store plugin name `{name}` is not a plain name"));
    }
    let repo_ok = repo.split('/').count() == 2
        && repo.split('/').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))
        });
    if !repo_ok {
        return Err(format!("`{name}`: repo `{repo}` is not owner/name"));
    }
    check_key(pubkey).map_err(|e| format!("`{name}`: {e}"))
}

/// A release tag goes into a URL path, so only what GitHub tags are made of.
fn check_tag(tag: &str) -> Result<(), String> {
    let ok = !tag.is_empty()
        && !tag.starts_with('.')
        && tag
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._+-".contains(ch));
    if ok {
        Ok(())
    } else {
        Err(format!("frozen tag `{tag}` is not a release tag"))
    }
}

fn check_key(b64: &str) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| "key is not base64".to_owned())?;
    if bytes.len() == 32 {
        Ok(())
    } else {
        Err("key is not 32 bytes".to_owned())
    }
}

/// Verify a store's signature against any of `trusted` public keys (the app
/// trusts two: the working store key and the cold backup), then parse it.
pub fn verify_store(json: &[u8], signature_b64: &str, trusted: &[&str]) -> Result<Store, String> {
    if !trusted
        .iter()
        .any(|k| crate::package::verify(json, signature_b64, k).is_ok())
    {
        return Err("the store's signature matches no trusted store key".to_owned());
    }
    Store::parse(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{generate_keypair, sign};

    fn store(plugin_key: &str, tier_key: &str) -> String {
        format!(
            r#"{{
              "version": 1,
              "tiers": {{
                "friendlyflow/cloud": {{ "issuer": "{tier_key}", "checkout": "https://x/checkout", "title": "store-tier-cloud" }}
              }},
              "plugins": [
                {{ "name": "notes", "repo": "friendlyflow/notes-plugin-sicompass",
                   "pubkey": "{plugin_key}", "category": "productivity",
                   "service": "friendlyflow/cloud", "revoked": ["ABCDEF"] }}
              ]
            }}"#
        )
    }

    #[test]
    fn a_signed_store_verifies_with_either_trusted_key() {
        let (_, plugin_pk) = generate_keypair().unwrap();
        let (_, tier_pk) = generate_keypair().unwrap();
        let json = store(&plugin_pk, &tier_pk);
        let (working, working_pk) = generate_keypair().unwrap();
        let (_, backup_pk) = generate_keypair().unwrap();
        let sig = sign(json.as_bytes(), &working).unwrap();

        let c = verify_store(json.as_bytes(), &sig, &[&backup_pk, &working_pk]).unwrap();
        let notes = c.entry("notes").unwrap();
        assert_eq!(
            notes.release_url("release.json"),
            "https://github.com/friendlyflow/notes-plugin-sicompass/releases/latest/download/release.json"
        );
        assert!(notes.is_revoked("abcdef"));
        assert!(!notes.is_revoked("123"));
        assert_eq!(notes.frozen, None);

        // Frozen: the tagged release, never the latest.
        let frozen = StoreEntry {
            frozen: Some("v1.4.0".to_owned()),
            ..notes.clone()
        };
        assert_eq!(
            frozen.release_url("release.json"),
            "https://github.com/friendlyflow/notes-plugin-sicompass/releases/download/v1.4.0/release.json"
        );
        assert_eq!(
            frozen.release_folder("http://127.0.0.1:1"),
            "http://127.0.0.1:1/friendlyflow/notes-plugin-sicompass/releases/download/v1.4.0/"
        );

        // Another key, or an edit after signing: refused.
        let (_, stranger) = generate_keypair().unwrap();
        assert!(verify_store(json.as_bytes(), &sig, &[&stranger]).is_err());
        let edited = json.replace("notes-plugin-sicompass", "evil_plugin");
        assert!(verify_store(edited.as_bytes(), &sig, &[&working_pk]).is_err());
    }

    #[test]
    fn structural_mistakes_are_named() {
        let (_, k) = generate_keypair().unwrap();
        let base = store(&k, &k);
        let bad =
            |from: &str, to: &str| Store::parse(base.replace(from, to).as_bytes()).unwrap_err();
        assert!(bad("\"version\": 1", "\"version\": 2").contains("format"));
        assert!(bad("friendlyflow/notes-plugin-sicompass", "no-slash").contains("owner/name"));
        assert!(bad("\"name\": \"notes\"", "\"name\": \"../x\"").contains("plain name"));
        assert!(bad(&format!("\"pubkey\": \"{k}\""), "\"pubkey\": \"short\"").contains("key"));
        assert!(
            bad(
                "\"service\": \"friendlyflow/cloud\"",
                "\"service\": \"nobody/tier\""
            )
            .contains("tier")
        );
        let frozen_at = |tag: &str| {
            base.replace(
                "\"revoked\"",
                &format!("\"frozen\": \"{tag}\", \"revoked\""),
            )
        };
        for tag in ["", "../x", ".hidden", "v1 2", "v1/2"] {
            let err = Store::parse(frozen_at(tag).as_bytes()).unwrap_err();
            assert!(err.contains("not a release tag"), "{tag}: {err}");
        }
        let c = Store::parse(frozen_at("v1.4.0-rc.1+b2").as_bytes()).unwrap();
        assert_eq!(
            c.entry("notes").unwrap().frozen.as_deref(),
            Some("v1.4.0-rc.1+b2")
        );
        Store::parse(base.as_bytes()).unwrap();
    }

    #[test]
    fn a_submission_is_checked_with_the_words_store_sign_uses() {
        let (_, k) = generate_keypair().unwrap();
        check_entry_fields("acme", "acme-dev/acme-plugin-sicompass", &k).unwrap();
        assert!(
            check_entry_fields("", "a/b", &k)
                .unwrap_err()
                .contains("plain name")
        );
        assert!(
            check_entry_fields("acme", "acme", &k)
                .unwrap_err()
                .contains("owner/name")
        );
        assert!(
            check_entry_fields("acme", "a/b/c", &k)
                .unwrap_err()
                .contains("owner/name")
        );
        assert!(
            check_entry_fields("acme", "a/b", "short")
                .unwrap_err()
                .contains("key")
        );
    }

    #[test]
    fn an_entry_that_is_not_frozen_writes_no_frozen_field() {
        let e = StoreEntry {
            name: "acme".into(),
            repo: "a/b".into(),
            pubkey: "k".into(),
            ..Default::default()
        };
        assert!(!serde_json::to_string(&e).unwrap().contains("frozen"));
    }
}
