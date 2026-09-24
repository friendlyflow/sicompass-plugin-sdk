//! `plugin.json`: the manifest every sicompass plugin ships.
//!
//! Portable (no `host` feature), because three parties read it: the app's plugin
//! host, the `sicompass-plugin` tool that packs and signs a release, and the
//! Store that shows a plugin before installing it. The rules for what a manifest
//! may ask for, and how a built component is checked against it, are in
//! [`crate::plugin_abi`]. The design is sicompass's docs/plugin-platform.md.

use serde::{Deserialize, Serialize};

/// How the plugin is executed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    /// Sandboxed WebAssembly component, and the only way to load third-party code.
    ///
    /// The default, so a manifest that omits `type` gets the sandbox rather than
    /// having to ask for it.
    #[default]
    Wasm,
    /// Instantiate a built-in factory provider by the manifest's `name` field.
    ///
    /// Not third-party code: this names a provider already compiled into the
    /// binary, which is why it is not sandboxed.
    Factory,
}

/// Plugin types that used to exist, kept only to explain their absence.
///
/// `native` loaded a `.so`/`.dll`/`.dylib` through `dlopen`, and `script` shelled
/// out to `bun`. Both are gone: Apple forbids executing downloaded native code and
/// equally forbids shipping a general-purpose interpreter, and a native in-process
/// plugin had full process privileges, so `allowedHosts` and every other manifest
/// policy was advisory against it. Neither could be made safe or shippable.
pub const RETIRED_TYPES: &[&str] = &["native", "script"];

/// Kind of a per-plugin setting entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    Text,
    Checkbox,
    Radio,
}

/// A single setting declared by a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSetting {
    #[serde(rename = "type")]
    pub kind: SettingKind,
    pub label: String,
    pub key: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub default_checked: bool,
    #[serde(default)]
    pub options: Vec<String>,
}

/// Parsed contents of a `plugin.json` manifest file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    pub display_name: String,
    #[serde(rename = "type", default)]
    pub plugin_type: PluginType,
    /// Relative entry path (resolved relative to the manifest directory).
    pub entry: String,
    #[serde(default)]
    pub supports_config_files: bool,
    #[serde(default)]
    pub settings: Vec<PluginSetting>,
    /// Optional plugin version, displayed in the settings tree under the
    /// plugin's section. Authors set this in `plugin.json`.
    #[serde(default)]
    pub version: Option<String>,
    /// HTTPS URL the updater queries for a newer manifest. Absent =>
    /// plugin opts out of auto-update.
    #[serde(default)]
    pub update_url: Option<String>,
    /// Minimum sicompass app version this plugin works against. If the
    /// running app is older, the plugin is skipped at load time.
    #[serde(default)]
    pub min_app_version: Option<String>,
    /// Base64-encoded ed25519 public key. Trust root for verifying
    /// signatures on future updates. First-install is trust-on-first-use.
    #[serde(default)]
    pub pubkey: Option<String>,
    /// Whether the running provider can be torn down + re-instantiated
    /// mid-session after an update lands on disk. Defaults to `true`;
    /// plugins that spawn long-lived threads holding fn-pointers from
    /// their own library must declare `false` and require a restart.
    #[serde(default = "default_hot_reload")]
    pub hot_reload: bool,
    /// Hosts a `wasm` plugin may reach over the network.
    ///
    /// This is a capability declaration, not a hint. Absent or empty means the
    /// network interface is **not linked into the guest at all**, so the plugin has
    /// no reachable network function rather than a blocked one. A component that
    /// uses the network without declaring hosts here is refused before it is
    /// instantiated.
    ///
    /// Matching is exact and case-insensitive: subdomains must be listed
    /// individually, because `evil.example.com` is not `example.com`. Listing them
    /// here is also what shows the user, before they enable the plugin, where it
    /// intends to connect.
    ///
    /// Also accepted as `permissions.allowedHosts`, where the other permissions
    /// live. The two lists are merged; see [`PluginManifest::allowed_hosts`].
    #[serde(default, rename = "allowedHosts")]
    pub top_level_allowed_hosts: Vec<String>,
    /// What the plugin may touch beyond the inert baseline every plugin gets.
    /// See docs/plugin-platform.md §4.
    #[serde(default)]
    pub permissions: Permissions,
    /// A Fluent id in the plugin's own `locales/`, describing it in one line for
    /// the Store.
    #[serde(default)]
    pub description: Option<String>,
    /// A paid service the plugin uses, shown before it is installed.
    #[serde(default)]
    pub service: Option<Service>,
}

/// `permissions` in `plugin.json`. Everything defaults to "not granted".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Permissions {
    /// Hosts the `net` interface may reach (same as the top-level key).
    pub allowed_hosts: Vec<String>,
    /// A folder of its own: `app_data_dir()/<name>`, preopened.
    pub storage: bool,
    /// Folders the user grants, preopened at the same path.
    pub filesystem: Vec<String>,
    /// Programs it may start.
    pub process: Vec<String>,
    /// `host:port` pairs it may open sockets to.
    pub sockets: Vec<String>,
}

/// `service` in `plugin.json`: a paid service on a server, which the Store shows
/// before install. The plugin itself stays free (docs/plugin-platform.md §1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Catalog tier id, for example `friendlyflow/cloud`.
    pub tier: String,
    /// A Fluent id in the plugin's `locales/`: what the service does.
    #[serde(default)]
    pub what: Option<String>,
}

impl PluginManifest {
    /// Hosts the plugin may reach: the top-level `allowedHosts` and
    /// `permissions.allowedHosts`, merged, in declaration order, without duplicates.
    pub fn allowed_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for h in self
            .top_level_allowed_hosts
            .iter()
            .chain(&self.permissions.allowed_hosts)
        {
            if !hosts.iter().any(|x| x.eq_ignore_ascii_case(h)) {
                hosts.push(h.clone());
            }
        }
        hosts
    }
}

fn default_hot_reload() -> bool {
    true
}

/// Parse a `plugin.json`. A retired plugin type (`native`, `script`) gets an error
/// that says so, instead of serde's "unknown variant".
pub fn parse_manifest(json: &str) -> Result<PluginManifest, String> {
    serde_json::from_str(json).map_err(|e| {
        let retired = serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned))
            .filter(|t| RETIRED_TYPES.contains(&t.as_str()));
        match retired {
            Some(t) => format!(
                "plugin type `{t}` is no longer supported: plugins are sandboxed \
                 WebAssembly components (`\"type\": \"wasm\"`)"
            ),
            None => e.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_parse_and_both_allowed_hosts_lists_merge() {
        let m = parse_manifest(
            r#"{
                "name": "notes", "displayName": "notes", "entry": "plugin.wasm",
                "allowedHosts": ["cloud.example.org"],
                "permissions": {
                    "allowedHosts": ["Cloud.example.org", "api.example.org"],
                    "storage": true
                },
                "description": "notes-description",
                "service": { "tier": "friendlyflow/cloud", "what": "notes-service" }
            }"#,
        )
        .unwrap();
        assert_eq!(
            m.allowed_hosts(),
            vec!["cloud.example.org".to_owned(), "api.example.org".to_owned()]
        );
        assert!(m.permissions.storage);
        assert_eq!(
            m.service.map(|s| s.tier).as_deref(),
            Some("friendlyflow/cloud")
        );
    }

    #[test]
    fn nothing_is_granted_by_default() {
        let m =
            parse_manifest(r#"{ "name": "x", "displayName": "x", "entry": "p.wasm" }"#).unwrap();
        assert_eq!(m.permissions, Permissions::default());
        assert!(m.allowed_hosts().is_empty());
        assert_eq!(m.plugin_type, PluginType::Wasm);
        assert!(m.hot_reload);
    }

    #[test]
    fn a_retired_type_is_explained() {
        let e = parse_manifest(
            r#"{ "name": "x", "displayName": "x", "entry": "x.so", "type": "native" }"#,
        )
        .unwrap_err();
        assert!(e.contains("no longer supported"), "{e}");
    }

    #[test]
    fn a_manifest_round_trips_through_json() {
        let json = r#"{ "name": "x", "displayName": "X", "entry": "plugin.wasm",
                        "version": "0.2.0", "permissions": { "storage": true } }"#;
        let m = parse_manifest(json).unwrap();
        let again = parse_manifest(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(m, again);
    }
}
